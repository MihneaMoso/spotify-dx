use dioxus::prelude::*;

/// A range input styled as a progress bar. Attaches `onscrub` to both `input`
/// (live) and `change` (committed) events.
#[component]
pub fn ProgressBar(position_ms: u64, duration_ms: u64, onscrub: EventHandler<u64>) -> Element {
    let max = if duration_ms > 0 { duration_ms } else { 1 };
    let value = position_ms.min(duration_ms);

    rsx! {
        input {
            class: "progress",
            r#type: "range",
            min: 0,
            max: "{max}",
            value: "{value}",
            aria_label: "Seek",
            oninput: move |evt| {
                if let Ok(ms) = evt.value().parse::<u64>() {
                    onscrub.call(ms.min(max));
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
            aria_label: "Volume",
            oninput: move |evt| {
                if let Ok(pct) = evt.value().parse::<u64>() {
                    onvolume.call((pct as f32).clamp(0.0, 100.0) / 100.0);
                }
            },
        }
    }
}