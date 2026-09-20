use dioxus::prelude::*;

/// A range input styled as a progress bar. Dragging previews locally via
/// `onpreview` (no backend traffic); releasing commits once via `onscrub`.
/// The old shape called one handler from both `input` and `change`, firing
/// two backend seeks per release.
#[component]
pub fn ProgressBar(
    position_ms: u64,
    duration_ms: u64,
    onpreview: EventHandler<u64>,
    onscrub: EventHandler<u64>,
) -> Element {
    let max = if duration_ms > 0 { duration_ms } else { 1 };
    let value = position_ms.min(duration_ms);
    // The stylesheet paints the played fill from `--fill` — without it the
    // track rendered permanently unfilled gray with only the thumb moving.
    let fill = value as f64 / max as f64 * 100.0;

    rsx! {
        input {
            class: "progress",
            r#type: "range",
            min: 0,
            max: "{max}",
            value: "{value}",
            style: "--fill: {fill:.1}%",
            aria_label: "Seek",
            oninput: move |evt| {
                if let Ok(ms) = evt.value().parse::<u64>() {
                    onpreview.call(ms.min(max));
                }
            },
            onchange: move |evt| {
                if let Ok(ms) = evt.value().parse::<u64>() {
                    onscrub.call(ms.min(max));
                }
            },
        }
    }
}

/// Volume slider, `0..=100` scaled to `0.0..=1.0`.
#[component]
pub fn VolumeBar(volume: f32, onvolume: EventHandler<f32>) -> Element {
    let value = (volume.clamp(0.0, 1.0) * 100.0).round() as u64;

    rsx! {
        input {
            class: "volume",
            r#type: "range",
            min: 0,
            max: 100,
            value: "{value}",
            style: "--fill: {value}%",
            aria_label: "Volume",
            oninput: move |evt| {
                if let Ok(pct) = evt.value().parse::<u64>() {
                    onvolume.call((pct as f32).clamp(0.0, 100.0) / 100.0);
                }
            },
        }
    }
}
