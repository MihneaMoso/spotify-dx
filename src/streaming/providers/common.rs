//! Shared provider plumbing: query encoding and HTTP client construction.
//! New providers use these directly; existing providers migrate
//! opportunistically. Bodies are behavior-preserving — `urlencode` is a
//! byte-identical move of the four copies that lived in
//! audius/piped/saavn/soundcloud.

/// Percent-encode a query component (`+` for space, RFC 3986 unreserved
/// set left bare). Matches all four former per-provider copies exactly.
pub fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else if b == b' ' {
            out.push('+');
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// House HTTP client: caller UA + bounded timeout (timeouts are cfg'd out
/// on wasm, where the browser owns the fetch — same pattern as the
/// per-provider builders this replaces for new code).
pub fn http_client(user_agent: &str, timeout_secs: u64) -> reqwest::Client {
    let builder = reqwest::Client::builder().user_agent(user_agent);
    #[cfg(not(target_arch = "wasm32"))]
    let builder = builder.timeout(std::time::Duration::from_secs(timeout_secs));
    builder.build().unwrap_or_default()
}

/// Fold diacritics to ASCII base letters for matching ("hoți"→"hoti",
/// "Beyoncé"→"beyonce"). Lowercases first; combining marks are dropped so
/// precomposed and decomposed spellings fold identically; scripts outside
/// the Latin table pass through (matching then behaves exactly as before
/// — strictly more recall, never less). Table covers Latin-1 Supplement +
/// Latin Extended-A/B (the artist/track-name space). Pure (unit-tested).
fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.to_lowercase().chars() {
        if ('\u{300}'..='\u{36f}').contains(&c) {
            continue;
        }
        match fold_char(c) {
            Some(rep) => out.push_str(rep),
            None => out.push(c),
        }
    }
    out
}

/// Base letters for a lowercased char; `None` = passthrough.
fn fold_char(c: char) -> Option<&'static str> {
    match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' | 'ǎ' | 'ȁ' | 'ȃ' => Some("a"),
        'æ' => Some("ae"),
        'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => Some("c"),
        'ď' | 'đ' | 'ð' => Some("d"),
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' | 'ȅ' | 'ȇ' | 'ȩ' => Some("e"),
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => Some("g"),
        'ĥ' | 'ħ' => Some("h"),
        'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' | 'ǐ' | 'ȉ' | 'ȋ' => Some("i"),
        'ĳ' => Some("ij"),
        'ĵ' => Some("j"),
        'ķ' | 'ĸ' => Some("k"),
        'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => Some("l"),
        'ñ' | 'ń' | 'ņ' | 'ň' | 'ŋ' => Some("n"),
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' | 'ơ' | 'ǒ' | 'ȍ' | 'ȏ' => Some("o"),
        'œ' => Some("oe"),
        'ŕ' | 'ŗ' | 'ř' => Some("r"),
        'ś' | 'ŝ' | 'ş' | 'š' | 'ș' => Some("s"),
        'ß' => Some("ss"),
        'ţ' | 'ť' | 'ŧ' | 'ț' => Some("t"),
        'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' | 'ư' | 'ǔ' | 'ȕ' | 'ȗ' => {
            Some("u")
        }
        'ŵ' => Some("w"),
        'ý' | 'ÿ' | 'ŷ' => Some("y"),
        'ź' | 'ż' | 'ž' => Some("z"),
        'þ' => Some("th"),
        _ => None,
    }
}

/// Normalized word tokens: folded alphanumeric runs, len ≥ 2
/// (single letters are noise: "B.E.N.Z" → b,e,n,z). Folding is identity
/// on plain ASCII, so existing behavior is unchanged there.
/// Pure (unit-tested).
pub fn word_tokens(s: &str) -> Vec<String> {
    fold(s)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() >= 2)
        .map(|w| w.to_string())
        .collect()
}

/// Squashed alphanumerics (folded): punctuation-blind comparison form
/// ("C*RVE, HOTI" → "crve hoti" → "crve hoti" without spaces). Pure.
fn squash(s: &str) -> String {
    fold(s).chars().filter(|c| c.is_alphanumeric()).collect()
}

/// Punctuation-stripped query form ("Curve, hoți" → "curve hoti"):
/// punctuation blinds YouTube-side search (a `Curve,` query returns zero
/// results where the bare words retrieve), so a sanitized variant rides
/// as the last-resort query. Folded + lowercased; spaces collapsed. Pure
/// (unit-tested).
pub fn sanitize_query(s: &str) -> String {
    fold(s)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Strips parenthetical/bracketed segments ("(Official Video)",
/// "[Remix]", "(feat. X)") — they describe the upload, not the song.
/// Unbalanced closers are kept (never eat the whole title). Pure.
pub fn strip_bracketed(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0u32;
    for c in s.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => {
                if depth == 0 {
                    out.push(c);
                } else {
                    depth -= 1;
                }
            }
            _ => {
                if depth == 0 {
                    out.push(c);
                }
            }
        }
    }
    out
}

/// Version words that mark a DIFFERENT recording than the track named:
/// an upload carrying one of these while the track title carries none is
/// never the song asked for (the Focu/instrumental case). Track-side
/// presence exempts (a track actually titled "… (Karaoke)" still matches).
/// Checked on the RAW video title — bracket-stripping would erase the
/// very signal. Pure (unit-tested).
const VERSION_WORDS: &[&str] = &[
    "instrumental",
    "instrumentals",
    "karaoke",
    "acapella",
    "a cappella",
    "backing track",
];

/// True when the candidate video plausibly IS the track: every track-title
/// word appears in the video title, and an artist word appears in the
/// title or author. Biased strict — a false reject tries the next
/// candidate, a false accept plays the wrong song. Diacritic spellings
/// fold together ("hoți" = "hoti"); deeper spelling drift needs the fuzzy
/// variant below. Pure (unit-tested).
pub fn title_matches(track_title: &str, artist: &str, video_title: &str, author: &str) -> bool {
    title_matches_inner(track_title, artist, video_title, author, false)
}

/// Fuzzy variant: exact gate first, else a 1-edit squash forgiveness for
/// spelling drift ("curve" vs "c*rve"/"curbe" — the Curve case, retrieved
/// first by search yet rejected by the exact gate). The caller MUST only
/// pass `true` when durations agree tightly (see `duration_tight`) —
/// without that anchor, drift forgiveness would admit different
/// recordings ("Focu" vs "Focus" is also 1 edit). Short squashes (< 8
/// chars) never fuzz: on few characters 1 edit proves nothing. The
/// version-word veto still runs first and always wins. Pure
/// (unit-tested).
pub fn title_matches_fuzzy(
    track_title: &str,
    artist: &str,
    video_title: &str,
    author: &str,
) -> bool {
    title_matches_inner(track_title, artist, video_title, author, true)
}

/// Fuzzy-match eligibility: spelling drift is forgiven only when
/// durations agree tightly. ±8s admits upload-length drift while
/// excluding different recordings (the Focu instrumental sits 9s off its
/// official — deliberately outside). Unknown durations never qualify.
/// Pure (unit-tested).
pub fn duration_tight(track_ms: u64, candidate_secs: Option<u64>) -> bool {
    match candidate_secs {
        Some(secs) if track_ms != 0 => track_ms.abs_diff(secs.saturating_mul(1000)) <= 8_000,
        _ => false,
    }
}

fn title_matches_inner(
    track_title: &str,
    artist: &str,
    video_title: &str,
    author: &str,
    fuzzy: bool,
) -> bool {
    // Version-word veto first: it decides on words the bracket-strip
    // would delete, so it must run on the raw titles.
    if version_mismatch(track_title, video_title) {
        return false;
    }
    let want: Vec<String> = word_tokens(&strip_bracketed(track_title));
    let title_toks = word_tokens(&strip_bracketed(video_title));
    if want.is_empty() {
        // Titles made only of sub-2-char tokens ("F&N" → f,n, both
        // filtered): compare squashed alphanumerics instead ("fn" ⊆
        // "futurefnaudio"). Single-char squashes stay rejected — a lone
        // letter matches everything and proves nothing.
        let needle = squash(track_title);
        if needle.len() < 2 {
            return false;
        }
        if !squash(video_title).contains(&needle) {
            return false;
        }
    } else if !want.iter().all(|w| title_toks.contains(w)) {
        // Exact words failed: forgive 1-edit spelling drift on the
        // punctuation-blind squash ("curvehotisilautari" vs
        // "…crvehotisilautari…"), caller-gated (see `title_matches_fuzzy`).
        if !fuzzy || !squash_contains_fuzzy(&squash(video_title), &squash(track_title)) {
            return false;
        }
    }
    let artist_toks = word_tokens(artist);
    if artist_toks.is_empty() {
        return true;
    }
    let author_toks = word_tokens(author);
    artist_toks
        .iter()
        .any(|a| title_toks.contains(a) || author_toks.contains(a))
}

/// Punctuation-blind containment with 1-edit forgiveness. Short needles
/// (< 8 chars) never match fuzzily — on few characters 1 edit is noise
/// ("focu" would forgive "focus"). Long needles accept exact containment
/// (strong signal) or any window within 1 edit. Pure (unit-tested).
fn squash_contains_fuzzy(hay: &str, needle: &str) -> bool {
    let h: Vec<char> = hay.chars().collect();
    let n: Vec<char> = needle.chars().collect();
    if n.len() < 8 || h.len() + 1 < n.len() {
        return false;
    }
    if hay.contains(needle) {
        return true;
    }
    for len in [n.len().saturating_sub(1), n.len(), n.len() + 1] {
        if len == 0 || len > h.len() {
            continue;
        }
        for w in h.windows(len) {
            if edit_le1(w, &n) {
                return true;
            }
        }
    }
    false
}

/// True when two char slices are ≤ 1 edit apart (insertion, deletion, or
/// substitution). Bounded DP with early exit — inputs here are title
/// fragments, tens of chars. Pure (unit-tested).
fn edit_le1(a: &[char], b: &[char]) -> bool {
    if a == b {
        return true;
    }
    let (n, m) = (a.len(), b.len());
    if n.abs_diff(m) > 1 {
        return false;
    }
    if n == 0 || m == 0 {
        return n + m <= 1;
    }
    let mut prev: Vec<usize> = (0..=m).collect();
    for i in 1..=n {
        let mut cur = vec![i; m + 1];
        let mut row_min = cur[0];
        for j in 1..=m {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
            row_min = row_min.min(cur[j]);
        }
        if row_min > 1 {
            return false;
        }
        prev = cur;
    }
    prev[m] <= 1
}

/// True when the video carries a version word the track lacks
/// ("Focu" vs "Focu (Instrumental)"). Token-based over lowercased text;
/// multiword entries ("a cappella", "backing track") match on the raw
/// substring. Pure (unit-tested).
fn version_mismatch(track_title: &str, video_title: &str) -> bool {
    // Tokenize first: version words overwhelmingly arrive bracketed
    // ("(Instrumental)"), where padding would never match. Multiword
    // entries ("a cappella", "backing track") survive via rejoin.
    let spaced = |s: &str| {
        s.to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    };
    let track = format!(" {} ", spaced(track_title));
    let video = format!(" {} ", spaced(video_title));
    // Padded matching both sides: "instrumentals" must not veto via the
    // "instrumental" entry (padding breaks the substring), and a track
    // that names the version itself is exempt.
    VERSION_WORDS.iter().any(|w| {
        let vw = format!(" {w} ");
        video.contains(&vw) && !track.contains(&vw)
    })
}

/// Fan-out across ranked query variants up to `cap` items: later variants
/// fill what earlier ones left thin (a junk-filled first variant must not
/// hide better later ones), duplicates dropped by `key`, order preserved.
/// Stops early once full, so the happy path still costs one fetch.
/// `Fetch::Abort` (transport death) stops everything and reports it; pure
/// async over injected fetches — unit-testable with canned results
/// (regression net for variant-stickiness bugs). Pure (unit-tested).
pub enum Fetch<T> {
    Items(Vec<T>),
    Abort,
}

pub async fn fanout_queries<'q, Q, T, K, F, Fut>(
    queries: &'q [Q],
    cap: usize,
    key: impl Fn(&T) -> K,
    mut fetch: F,
) -> (Vec<T>, bool)
where
    K: Eq + std::hash::Hash,
    F: FnMut(&'q Q) -> Fut,
    Fut: std::future::Future<Output = Fetch<T>>,
{
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for q in queries {
        if out.len() >= cap {
            break;
        }
        match fetch(q).await {
            Fetch::Abort => return (out, true),
            Fetch::Items(items) => {
                for item in items {
                    if out.len() >= cap {
                        break;
                    }
                    if seen.insert(key(&item)) {
                        out.push(item);
                    }
                }
            }
        }
    }
    (out, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urlencode_escapes_query_chars() {
        assert_eq!(urlencode("Kanye West"), "Kanye+West");
        assert_eq!(urlencode("R&B/Hip-Hop"), "R%26B%2FHip-Hop");
        assert_eq!(urlencode("abc-_.~09"), "abc-_.~09");
    }

    #[test]
    fn tokens_normalize_and_drop_noise() {
        assert_eq!(
            word_tokens("Satra B.E.N.Z., Radu Guran"),
            vec!["satra", "radu", "guran"]
        );
        assert_eq!(word_tokens("OU, OU"), vec!["ou", "ou"]);
        assert_eq!(word_tokens("a"), Vec::<String>::new());
    }

    #[test]
    fn brackets_stripped_without_eating_title() {
        assert_eq!(
            strip_bracketed("Furam Curent (ZYKRØ Remix)").trim(),
            "Furam Curent"
        );
        assert_eq!(strip_bracketed("Song [Official Video]").trim(), "Song");
        assert_eq!(strip_bracketed("Plain Title"), "Plain Title");
        assert_eq!(strip_bracketed("A (B] C"), "A  C");
    }

    #[test]
    fn observed_ranking_official_accepted_rest_rejected() {
        // Real f5.si ranking for the exact app query (Oct 2026).
        const TRACK: &str = "FURAM CURENT";
        const ARTIST: &str = "Satra B.E.N.Z., Radu Guran";
        let official = "Satra B.E.N.Z. & Radu Guran - Furam Curent (Official Video)";
        assert!(title_matches(TRACK, ARTIST, official, "Satra B.E.N.Z."));
        // Remix shares tokens but the duration gate handles it downstream.
        let remix = "⚡ Satra B.E.N.Z. & Radu Guran - Furam Curent (ZYKRØ Remix)";
        assert!(title_matches(TRACK, ARTIST, remix, "ZYKRØ"));
        // Same-artist different songs share no title tokens: rejected.
        assert!(!title_matches(
            TRACK,
            ARTIST,
            "Radu Guran X Fane Alu Brigitte - Bombardieri Sensibili",
            "Radu Guran"
        ));
        assert!(!title_matches(TRACK, ARTIST, "OU, OU", "Lx1dd2poLV4"));
        assert!(!title_matches(
            TRACK,
            ARTIST,
            "Satra B.E.N.Z. - Satra Se Intoarce Din Nou (Official Video)",
            "Satra B.E.N.Z."
        ));
    }

    #[test]
    fn near_identical_titles_rejected() {
        // Proven on-device: a 186s wrong song served for a 173s track,
        // 12.5s inside the duration tolerance ("amarata" ≠ "amărăți").
        assert!(!title_matches(
            "E Amarata",
            "Jean Gaoaza",
            "Jean Gaoaza - Bombardierii Amărâți (Official Video)",
            "Jean Gaoaza"
        ));
        assert!(title_matches(
            "E Amarata",
            "Jean Gaoaza",
            "Jean Gaoaza - E Amarata (Official Video)",
            "Jean Gaoaza"
        ));
    }

    #[test]
    fn lyric_video_without_artist_rejected() {
        // Strict-biased: title-only upload on a generic channel misses the
        // artist check; fallthrough continues elsewhere rather than
        // risking a wrong song.
        assert!(!title_matches(
            "FURAM CURENT",
            "Satra B.E.N.Z., Radu Guran",
            "Furam Curent",
            "Lyrics Zone"
        ));
    }

    #[test]
    fn empty_track_never_matches() {
        assert!(!title_matches(
            "",
            "Satra B.E.N.Z.",
            "Furam Curent",
            "Satra B.E.N.Z."
        ));
    }

    #[tokio::test]
    async fn fanout_fills_thin_variants_and_stops_when_full() {
        // Thin first variant pulls from later ones (the FURAM recall bug:
        // junk-filled variant 1 hid better variants 2/3).
        let (out, aborted): (Vec<String>, bool) = fanout_queries(
            &["q1".to_string(), "q2".to_string(), "q3".to_string()],
            6,
            |s: &String| s.clone(),
            |q: &String| async move {
                Fetch::Items(match q.as_str() {
                    "q1" => vec!["a".to_string()],
                    "q2" => vec!["b".to_string(), "c".to_string()],
                    _ => vec![],
                })
            },
        )
        .await;
        assert_eq!(out, vec!["a", "b", "c"]);
        assert!(!aborted);
    }

    #[tokio::test]
    async fn fanout_stops_at_cap_without_extra_fetches() {
        // Full first variant: later variants never run (call-counted).
        let calls = std::cell::Cell::new(0);
        let calls_ref = &calls;
        let (out, aborted): (Vec<String>, bool) = fanout_queries(
            &[
                "q1".to_string(),
                "q2".to_string(),
            ],
            2,
            |s: &String| s.clone(),
            |q: &String| async move {
                calls_ref.set(calls_ref.get() + 1);
                Fetch::Items(match q.as_str() {
                    "q1" => vec!["a".to_string(), "b".to_string()],
                    _ => vec!["c".to_string()],
                })
            },
        )
        .await;
        assert_eq!(out, vec!["a", "b"]);
        assert!(!aborted);
        assert_eq!(calls.get(), 1);
    }

    #[tokio::test]
    async fn fanout_dedupes_across_variants_in_order() {
        let (out, aborted): (Vec<String>, bool) = fanout_queries(
            &["q1".to_string(), "q2".to_string()],
            6,
            |s: &String| s.clone(),
            |q: &String| async move {
                Fetch::Items(match q.as_str() {
                    "q1" => vec!["a".to_string(), "b".to_string()],
                    _ => vec!["b".to_string(), "c".to_string()],
                })
            },
        )
        .await;
        assert_eq!(out, vec!["a", "b", "c"]);
        assert!(!aborted);
    }

    #[tokio::test]
    async fn fanout_abort_keeps_accumulated_and_reports() {
        // Transport death mid-fanout: keep variant-1 items, report abort
        // so the caller can cool the host (Invidious semantics).
        let (out, aborted): (Vec<String>, bool) = fanout_queries(
            &["q1".to_string(), "q2".to_string(), "q3".to_string()],
            6,
            |s: &String| s.clone(),
            |q: &String| async move {
                match q.as_str() {
                    "q1" => Fetch::Items(vec!["a".to_string()]),
                    "q2" => Fetch::Abort,
                    _ => Fetch::Items(vec!["c".to_string()]),
                }
            },
        )
        .await;
        assert_eq!(out, vec!["a"]);
        assert!(aborted);
    }

    /// Regression (Focu/instrumental): a version word the track lacks
    /// vetoes, even though bracket-stripping makes the titles identical.
    #[test]
    fn version_words_veto_wrong_recordings() {
        // Official passes.
        assert!(title_matches(
            "Focu", "Ian",
            "IAN x AZTECA - FOCU'", "Ocult Records"
        ));
        // Instrumental / karaoke / acapella fail for a plain title…
        for v in [
            "Ian x Azteca - Focu' (Instrumental)",
            "Focu (Karaoke Version) - Sing King",
            "Focu - Acapella Vocals Only",
            "Focu (Backing Track)",
        ] {
            assert!(!title_matches("Focu", "Ian", v, "Someone"), "veto: {v}");
        }
        // …but pass when the TRACK itself names the version.
        assert!(title_matches(
            "Focu (Instrumental)", "Ian",
            "Ian - Focu (Instrumental)", "Ian"
        ));
        // Unrelated version words don't veto (real-world shape: the
        // artist rides in the title when the channel name lacks it).
        assert!(title_matches("Focu", "Ian", "IAN x AZTECA - FOCU' (Official Audio)", "Ocult Records"));
    }

    /// Regression (F&N): titles made only of sub-2-char tokens could never
    /// match (`word_tokens` filters len<2, leaving `want` empty → false).
    /// Squashed-alphanumeric fallback covers them.
    #[test]
    fn short_token_titles_match_by_squash() {
        assert!(title_matches(
            "F&N", "Future",
            "Future - F&N (Audio)", "FutureVEVO"
        ));
        // Single letters still prove nothing.
        assert!(!title_matches("E", "Someone", "Everything", "Someone"));
        // Squash must actually appear.
        assert!(!title_matches("F&N", "Future", "Future - Mask Off", "FutureVEVO"));
    }

    /// Diacritics fold: Spotify's spelling and the upload's spelling meet
    /// halfway instead of mismatching.
    #[test]
    fn diacritics_fold_together() {
        assert_eq!(fold("HOȚI ȘI"), "hoti si");
        assert_eq!(fold("Beyoncé"), "beyonce");
        assert_eq!(fold("Curbe"), "curbe");
        assert!(title_matches(
            "Hoți Si Lautari", "Radu Guran",
            "Radu Guran - HoTi Si Lautari", "Radu Guran"
        ));
    }

    /// Regression (Curve): 1-letter spelling drift is forgiven in fuzzy
    /// mode — exact still rejects, fuzzy admits.
    #[test]
    fn fuzzy_forgives_single_letter_drift() {
        let track = "CURVE, HOTI SI LAUTARI";
        let video = "Radu Guran ❌ Daniel Vots - C*RVE, HOTI SI LAUTARI | Official Video";
        assert!(!title_matches(track, "Radu Guran", video, "Radu Guran"));
        assert!(title_matches_fuzzy(track, "Radu Guran", video, "Radu Guran"));
        // Same for the curbe spelling.
        assert!(title_matches_fuzzy(
            track, "Radu Guran",
            "Radu Guran - Curbe Hoti Si Lautari", "Radu Guran"
        ));
    }

    /// The Focu/Focus guard: short squashes never fuzz, and the veto still
    /// wins even in fuzzy mode.
    #[test]
    fn fuzzy_stays_strict_on_short_and_versioned() {
        // "focu" (4 chars) vs "focus…": no fuzzy forgiveness.
        assert!(!title_matches_fuzzy("Focu", "Ian", "Focus - Single", "Someone"));
        // Artist must still match.
        assert!(!title_matches_fuzzy(
            "CURVE, HOTI SI LAUTARI", "Radu Guran",
            "C*RVE, HOTI SI LAUTARI", "Random Channel"
        ));
        // Version words veto before fuzziness is even considered.
        assert!(!title_matches_fuzzy(
            "Focu", "Ian",
            "Ian - Focu' (Instrumental)", "Ian"
        ));
        // Two edits is too much drift.
        assert!(!title_matches_fuzzy(
            "Curve Hoti Si Lautari", "Radu Guran",
            "Radu Guran - XYZVE HOTI SI LAUTARI", "Radu Guran"
        ));
    }

    /// Tight-duration rule: ±8s admits upload drift, 9s+ and unknowns fail.
    #[test]
    fn duration_tight_bands_correctly() {
        assert!(duration_tight(154_000, Some(160))); // Curve video: 6s off
        assert!(duration_tight(163_000, Some(163)));
        assert!(!duration_tight(163_000, Some(172))); // instrumental: 9s off
        assert!(!duration_tight(163_000, None));
        assert!(!duration_tight(0, Some(163)));
    }

    #[test]
    fn edit_le1_grades_correctly() {
        assert!(edit_le1(&chars("curve"), &chars("crve"))); // deletion
        assert!(edit_le1(&chars("curve"), &chars("curbe"))); // substitution
        assert!(edit_le1(&chars("focu"), &chars("focus"))); // insertion
        assert!(edit_le1(&chars("same"), &chars("same")));
        assert!(!edit_le1(&chars("curve"), &chars("xyzve"))); // 2+ edits
        assert!(!edit_le1(&chars("curve"), &chars("cur"))); // len diff 2
    }

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    /// Query sanitizer: punctuation and diacritics go, words stay.
    #[test]
    fn sanitize_query_strips_punctuation() {
        assert_eq!(sanitize_query("Curve, hoți si lăutari"), "curve hoti si lautari");
        assert_eq!(sanitize_query("F&N"), "f n");
        assert_eq!(sanitize_query("  Mercy  "), "mercy");
    }
}
