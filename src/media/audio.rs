//! Phase-0 audio-stack spike: decode compressed audio (FLAC today; AAC/M4A,
//! MP3, Ogg enabled in the feature set) straight from bytes with **symphonia**,
//! proving the three operations the open playback engine needs
//! (SYSTEM_DESIGN §6.3):
//!
//! 1. decode from an in-memory/streamed source (no temp files),
//! 2. accurate seek inside the decoded timeline,
//! 3. deterministic timing metadata for gapless hand-off between tracks.
//!
//! Playback (the sound-card side) is rodio's job in Phase 4b; this module
//! deliberately stays output-free so it is testable headless.
//!
//! Verified against tiny ffmpeg-generated fixtures under
//! `assets/test-audio/` (`include_bytes!`, network-free).

use std::io::{Cursor, Read, Seek};

use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::codecs::CodecParameters;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::{Time, TimeBase};

/// Errors surfaced by the spike. The real engine gets richer variants in
/// Phase 4b; these map 1:1 onto what the resolver/sink layer needs to know.
#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("unsupported or unrecognized audio format")]
    Unsupported,
    #[error("no decodable audio track found")]
    NoTrack,
    #[error("decode failed: {0}")]
    Decode(String),
}

/// An owned byte source satisfying symphonia's `MediaSource` without relying
/// on blanket impls (stable across symphonia minor versions).
struct BytesSource {
    inner: Cursor<Vec<u8>>,
}

impl BytesSource {
    fn new(data: Vec<u8>) -> Self {
        Self {
            inner: Cursor::new(data),
        }
    }
}

impl Read for BytesSource {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.inner.read(buf)
    }
}

impl Seek for BytesSource {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

impl MediaSource for BytesSource {
    fn is_seekable(&self) -> bool {
        true
    }
    fn byte_len(&self) -> Option<u64> {
        Some(self.inner.get_ref().len() as u64)
    }
}

/// One decoded frame ("packet" in symphonia terms): enough timing metadata to
/// drive the progress bar and the gapless planner without copying samples
/// around yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameInfo {
    /// Timestamp of the first sample in this frame, milliseconds.
    pub first_sample_ms: u64,
    /// Number of valid sample frames in this packet.
    pub sample_frames: u32,
}

/// An opened, seekable, decodable track.
pub struct TrackDecoder {
    format_reader: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    sample_rate: u32,
    total_frames: Option<u64>,
    finished: bool,
}

impl TrackDecoder {
    /// Open an encoded track from memory. `hint_ext` (e.g. `"flac"`,
    /// `"m4a"`) seeds format detection; probing still sniffs content first.
    pub fn from_bytes(data: Vec<u8>, hint_ext: &str) -> Result<Self, AudioError> {
        let mut hint = Hint::new();
        hint.with_extension(hint_ext);
        let mss = MediaSourceStream::new(Box::new(BytesSource::new(data)), Default::default());
        // Owned source ⇒ 'static; the probed reader never outlives us.
        Self::open(mss, hint)
    }

    fn open(mss: MediaSourceStream<'static>, hint: Hint) -> Result<Self, AudioError> {
        // get_probe()/get_codecs() pre-register every feature-enabled format
        // and codec (flac/aac/mp3/isomp4/ogg/pcm in our Cargo features).
        let format_reader: Box<dyn FormatReader> = symphonia::default::get_probe()
            .probe(
                &hint,
                mss,
                FormatOptions::default(),
                MetadataOptions::default(),
            )
            .map_err(|_| AudioError::Unsupported)?;

        let track = format_reader
            .tracks()
            .iter()
            .find(|t| t.codec_params.as_ref().is_some_and(CodecParameters::is_audio))
            .ok_or(AudioError::NoTrack)?;

        let track_id = track.id;
        let audio_params = track
            .codec_params
            .as_ref()
            .and_then(CodecParameters::audio)
            .ok_or(AudioError::NoTrack)?;
        let sample_rate = audio_params.sample_rate.ok_or(AudioError::NoTrack)?;
        let total_frames = track.num_frames;

        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(audio_params, &AudioDecoderOptions::default())
            .map_err(|e| AudioError::Decode(e.to_string()))?;

        Ok(Self {
            format_reader,
            decoder,
            track_id,
            sample_rate,
            total_frames,
            finished: false,
        })
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Total duration in milliseconds, from container metadata when present.
    pub fn duration_ms(&self) -> Option<u64> {
        self.total_frames
            .map(|frames| frames * 1000 / u64::from(self.sample_rate))
    }

    /// Accurate seek to `ms` on the decoded timeline.
    pub fn seek_ms(&mut self, ms: u64) -> Result<(), AudioError> {
        let time = Time::try_from_nanos_u128(u128::from(ms) * 1_000_000)
            .ok_or_else(|| AudioError::Decode("seek target out of range".into()))?;
        self.format_reader
            .seek(
                SeekMode::Accurate,
                SeekTo::Time {
                    time,
                    track_id: Some(self.track_id),
                },
            )
            .map_err(|e| AudioError::Decode(e.to_string()))?;
        // Decoder-side state must be dropped after an upstream seek.
        self.decoder.reset();
        Ok(())
    }

    fn track_timebase(&self) -> Option<TimeBase> {
        self.format_reader
            .tracks()
            .iter()
            .find(|t| t.id == self.track_id)
            .and_then(|t| t.time_base)
    }

    /// Decode the next frame. Returns `None` cleanly at end-of-stream (and
    /// stays exhausted afterwards).
    pub fn decode_next(&mut self) -> Result<Option<FrameInfo>, AudioError> {
        if self.finished {
            return Ok(None);
        }
        loop {
            let packet = match self.format_reader.next_packet() {
                Ok(Some(packet)) => packet,
                Ok(None) => {
                    self.finished = true;
                    return Ok(None);
                }
                Err(e) => return Err(AudioError::Decode(e.to_string())),
            };
            if packet.track_id != self.track_id {
                continue; // attached pictures / other streams
            }
            // Read the timebase BEFORE decoding: the decoded buffer borrows
            // the decoder mutably until it's consumed below.
            let first_sample_ms = self.track_timebase().map_or(0, |tb| {
                // ms = ts_ticks * tb.numer * 1000 / tb.denom (u128: no overflow)
                ((u128::try_from(packet.pts.get()).unwrap_or(0)
                    * u128::from(tb.numer.get())
                    * 1000)
                    / u128::from(tb.denom.get())) as u64
            });
            let decoded = self
                .decoder
                .decode(&packet)
                .map_err(|e| AudioError::Decode(e.to_string()))?;

            return Ok(Some(FrameInfo {
                first_sample_ms,
                sample_frames: decoded.frames() as u32,
            }));
        }
    }
}

/// Gapless hand-off scheduler (pure logic, no audio): decides when the sink
/// must start resolving/decoding the next track so playback never gaps.
///
/// This is the policy Phase 4b's sink will poll each tick.
#[derive(Debug, Clone, Copy)]
pub struct GaplessPlanner {
    /// How long before end-of-track the next one must be ready.
    pub prebuffer_lead_ms: u64,
}

impl Default for GaplessPlanner {
    fn default() -> Self {
        Self {
            prebuffer_lead_ms: 10_000,
        }
    }
}

impl GaplessPlanner {
    /// Start prebuffering when we are within the lead window of the end.
    pub fn should_prebuffer_next(&self, position_ms: u64, duration_ms: u64) -> bool {
        duration_ms > 0 && position_ms + self.prebuffer_lead_ms >= duration_ms
    }

    /// Position the UI reports during a hand-off window: clamp to the track
    /// duration so the progress bar never runs past 100%.
    pub fn display_position_ms(&self, position_ms: u64, duration_ms: u64) -> u64 {
        if duration_ms == 0 {
            position_ms
        } else {
            position_ms.min(duration_ms)
        }
    }
}

// ── fixtures ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const TONE_FLAC: &[u8] = include_bytes!("../../assets/test-audio/tone.flac");
    const TONE_M4A: &[u8] = include_bytes!("../../assets/test-audio/tone.m4a");

    const EXPECTED_MS: u64 = 600;
    const TOLERANCE_MS: u64 = 80;

    #[test]
    fn decodes_flac_from_bytes() {
        let mut dec = TrackDecoder::from_bytes(TONE_FLAC.to_vec(), "flac").unwrap();
        let dur = dec.duration_ms().expect("flac carries num_frames");
        assert!(
            dur.abs_diff(EXPECTED_MS) <= TOLERANCE_MS,
            "duration {dur}ms not ~{EXPECTED_MS}ms"
        );
        assert!(
            (8000..=192_000).contains(&dec.sample_rate()),
            "implausible sample rate {}",
            dec.sample_rate()
        );

        // Decode a healthy chunk end-to-end.
        let mut frames = 0;
        let mut last_ts = 0;
        while let Some(info) = dec.decode_next().unwrap() {
            assert!(info.sample_frames > 0);
            last_ts = info.first_sample_ms;
            frames += 1;
            if frames >= 10 {
                break;
            }
        }
        assert_eq!(frames, 10, "decoded only {frames} frames");
        assert!(last_ts < dur, "timestamps ran past duration");

        // Stream exhausts cleanly to None.
        while dec.decode_next().unwrap().is_some() {}
        assert_eq!(dec.decode_next().unwrap(), None);
    }

    #[test]
    fn decodes_aac_m4a_from_bytes() {
        let mut dec = TrackDecoder::from_bytes(TONE_M4A.to_vec(), "m4a").unwrap();
        if let Some(dur) = dec.duration_ms() {
            assert!(
                dur.abs_diff(EXPECTED_MS) <= TOLERANCE_MS * 2,
                "m4a duration {dur}ms not ~{EXPECTED_MS}ms"
            );
        }
        // Decoding itself must succeed regardless of metadata completeness.
        let mut saw_any = false;
        while let Some(info) = dec.decode_next().unwrap() {
            saw_any = true;
            assert!(info.first_sample_ms <= EXPECTED_MS * 3);
        }
        assert!(saw_any, "m4a decoded zero frames");
    }

    #[test]
    fn seeks_accurately_in_flac() {
        let mut dec = TrackDecoder::from_bytes(TONE_FLAC.to_vec(), "flac").unwrap();
        dec.seek_ms(EXPECTED_MS / 2).unwrap();
        let info = dec.decode_next().unwrap().expect("frame after seek");
        let target = EXPECTED_MS / 2;
        assert!(
            info.first_sample_ms.abs_diff(target) <= TOLERANCE_MS,
            "seeked to {}ms, wanted ~{target}ms",
            info.first_sample_ms
        );
    }

    #[test]
    fn rejects_non_audio_bytes() {
        let junk = b"this is definitely not audio".repeat(64);
        assert!(matches!(
            TrackDecoder::from_bytes(junk, ""),
            Err(AudioError::Unsupported | AudioError::NoTrack)
        ));
    }

    #[test]
    fn gapless_planner_windows_are_sane() {
        let p = GaplessPlanner::default();
        // Far from the end: don't prebuffer.
        assert!(!p.should_prebuffer_next(30_000, 180_000));
        // Inside the 10 s window: do.
        assert!(p.should_prebuffer_next(171_000, 180_000));
        // Zero duration (live/unknown): never.
        assert!(!p.should_prebuffer_next(999_999, 0));
        // Display clamp keeps the progress bar inside the track.
        assert_eq!(p.display_position_ms(181_000, 180_000), 180_000);
        assert_eq!(p.display_position_ms(50_000, 180_000), 50_000);
    }
}



