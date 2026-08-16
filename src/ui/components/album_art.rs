use dioxus::prelude::*;

use crate::spotify::client;

/// Colored placeholder shown while artwork loads.
pub fn color_from_seed(seed: &str) -> String {
    let hash = fnv1a(seed);
    let hue = hash % 360;
    let (r, g, b) = hsl_to_rgb(hue, 42, 30);
    format!("#{r:02x}{g:02x}{b:02x}")
}

fn fnv1a(s: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in s.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn hsl_to_rgb(h: u64, s: u64, l: u64) -> (u8, u8, u8) {
    let h = h as f32 / 360.0;
    let s = s as f32 / 100.0;
    let l = l as f32 / 100.0;
    let (r, g, b);
    if s == 0.0 {
        (r, g, b) = (l, l, l);
    } else {
        let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
        let p = 2.0 * l - q;
        r = hue_to_rgb(p, q, h + 1.0 / 3.0);
        g = hue_to_rgb(p, q, h);
        b = hue_to_rgb(p, q, h - 1.0 / 3.0);
    }
    (
        (r * 255.0).round() as u8,
        (g * 255.0).round() as u8,
        (b * 255.0).round() as u8,
    )
}

fn hue_to_rgb(p: f32, q: f32, t: f32) -> f32 {
    let mut t = t;
    if t < 0.0 {
        t += 1.0;
    }
    if t > 1.0 {
        t -= 1.0;
    }
    if t < 1.0 / 6.0 {
        p + (q - p) * 6.0 * t
    } else if t < 0.5 {
        q
    } else if t < 2.0 / 3.0 {
        p + (q - p) * (2.0 / 3.0 - t) * 6.0
    } else {
        p
    }
}

/// Async artwork loading state.
#[derive(Debug, Clone, Default)]
enum Artwork {
    #[default]
    Loading,
    Ready {
        /// A heavily downscaled blur-up version.
        blurred: String,
        /// The full-resolution data URI.
        full: String,
    },
}

/// Fetch artwork bytes over the ad-filtered client and hand back a data URI.
/// Runs entirely on the UI thread (inside a dioxus `spawn`), so it never
/// blocks rendering; while it loads we show a deterministic colored div.
fn use_artwork(url: String) -> Signal<Artwork> {
    let mut state = use_signal(Artwork::default);
    use_effect(move || {
        if url.is_empty() {
            return;
        }
        let url = url.clone();
        dioxus::prelude::spawn(async move {
            if let Ok(bytes) = load_image_bytes(&url).await {
                let (blurred, full) = encode_blur_and_full(&bytes);
                state.set(Artwork::Ready { blurred, full });
            }
        });
    });
    state
}

async fn load_image_bytes(url: &str) -> Result<Vec<u8>, crate::app_error::AppError> {
    let resp = client::filtered_get(url).await?;
    resp.error_for_status()?
        .bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(crate::app_error::AppError::Network)
}

/// Build the blur-up (32px, heavy quality loss) and full (< 512px) JPEG data URIs.
fn encode_blur_and_full(bytes: &[u8]) -> (String, String) {
    use base64::Engine as _;
    let Ok(img) = image::load_from_memory(bytes) else {
        // Non-image payload: serve the raw bytes as-is.
        let mime = infer_mime(bytes);
        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
        return (String::new(), format!("data:{mime};base64,{encoded}"));
    };

    let full_image = img.resize(512, 512, image::imageops::FilterType::Triangle);
    let blur_image = img.resize(24, 24, image::imageops::FilterType::Triangle);
    (
        encode_jpeg(&blur_image),
        encode_jpeg(&full_image),
    )
}

fn encode_jpeg(img: &image::DynamicImage) -> String {
    use base64::Engine as _;
    let rgb = img.to_rgb8();
    let mut buf = Vec::new();
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 70);
    if enc.encode_image(&rgb).is_ok() {
        let encoded = base64::engine::general_purpose::STANDARD.encode(&buf);
        format!("data:image/jpeg;base64,{encoded}")
    } else {
        String::new()
    }
}

fn infer_mime(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(b"\x89PNG") {
        "image/png"
    } else if bytes.starts_with(b"\xff\xd8") {
        "image/jpeg"
    } else if bytes.starts_with(b"RIFF") && bytes.len() > 12 && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else {
        "application/octet-stream"
    }
}

/// Lazy-loaded album art with a colored placeholder and blur-up swap-in.
#[component]
pub fn AlbumArt(url: String, seed: String, class: Option<String>) -> Element {
    let art = use_artwork(url);
    let class = class.unwrap_or_default();
    let state = match &*art.read() {
        Artwork::Loading => {
            let bg = color_from_seed(&seed);
            rsx! {
                div { class: "album-art placeholder {class}", style: "background-color: {bg};" }
            }
        }
        Artwork::Ready { blurred, full } => {
            let blur_style = if blurred.is_empty() {
                String::new()
            } else {
                format!("background-image: url({blurred}); background-size: cover;")
            };
            rsx! {
                div { class: "album-art {class}", style: "{blur_style}",
                    img { class: "album-art-img", src: "{full}", loading: "lazy" }
                }
            }
        }
    };
    let _ = art;
    state
}