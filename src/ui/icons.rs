use dioxus::prelude::*;

/// Inline SVG icons. Kept in one module so the player bar / nav can share them
/// without shipping binary assets (SVG-as-data-URI would defeat the file engine).
pub fn icon(svg: Element, size: i32) -> Element {
    rsx! {
        span { class: "icon", width: "{size}", height: "{size}",
            {svg}
        }
    }
}

pub fn play(size: i32) -> Element {
    rsx! {
        svg { width: "{size}", height: "{size}", view_box: "0 0 24 24", fill: "currentColor",
            path { d: "M8 5v14l11-7z" }
        }
    }
}

pub fn pause(size: i32) -> Element {
    rsx! {
        svg { width: "{size}", height: "{size}", view_box: "0 0 24 24", fill: "currentColor",
            path { d: "M6 5h4v14H6zM14 5h4v14h-4z" }
        }
    }
}

pub fn skip_forward(size: i32) -> Element {
    rsx! {
        svg { width: "{size}", height: "{size}", view_box: "0 0 24 24", fill: "currentColor",
            path { d: "M4 5v14l8-7zM16 5v14h2.5V5z" }
        }
    }
}

pub fn skip_back(size: i32) -> Element {
    rsx! {
        svg { width: "{size}", height: "{size}", view_box: "0 0 24 24", fill: "currentColor",
            path { d: "M20 5v14l-8-7zM5.5 5H8v14H5.5z" }
        }
    }
}

pub fn shuffle(size: i32, active: bool) -> Element {
    let color = if active { "#60a5fa" } else { "currentColor" };
    rsx! {
        svg { width: "{size}", height: "{size}", view_box: "0 0 24 24", fill: "{color}",
            path { d: "M10.6 9.5 7.1 6 4 9.1 2.9 8 6 4.9 3.5 2.4 4.6 1.3 9.4 6.1 8.3 7.2 7.1 6z M2 11l.6 1L4 9.9 5.4 8.4 4.4 7.4z" }
            path { d: "M4 4h4.5v2H4zM16.5 4H20a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2h-3.5v-2H20V6h-3.5z" }
            path { d: "M4 20v-4.5h2V20z" }
        }
    }
}

pub fn repeat(size: i32, active: bool) -> Element {
    let color = if active { "#60a5fa" } else { "currentColor" };
    rsx! {
        svg { width: "{size}", height: "{size}", view_box: "0 0 24 24", fill: "{color}",
            path { d: "M7 7h10v3l4-4-4-4v3H5v6h2V7zm10 10H7v-3l-4 4 4 4v-3h12v-6h-2v4z" }
        }
    }
}

pub fn heart(size: i32, active: bool) -> Element {
    let color = if active { "#60a5fa" } else { "currentColor" };
    rsx! {
        svg { width: "{size}", height: "{size}", view_box: "0 0 24 24", fill: "{color}",
            path { d: "M12 21s-8-4.6-10-9.3C.9 8.4 3.4 5 7 5c2 0 3.4 1 5 2.8C13.6 6 15 5 17 5c3.6 0 6.1 3.4 5 6.7C20 16.4 12 21 12 21z" }
        }
    }
}

pub fn home(size: i32) -> Element {
    rsx! {
        svg { width: "{size}", height: "{size}", view_box: "0 0 24 24", fill: "currentColor",
            path { d: "M12 3l9 8h-3v9h-4v-6h-4v6H6v-9H3z" }
        }
    }
}

pub fn search(size: i32) -> Element {
    rsx! {
        svg { width: "{size}", height: "{size}", view_box: "0 0 24 24", fill: "none",
            stroke: "currentColor", stroke_width: "2",
            circle { cx: "10.5", cy: "10.5", r: "6.5" }
            path { d: "M15.5 15.5 21 21", stroke: "currentColor" }
        }
    }
}

pub fn library(size: i32) -> Element {
    rsx! {
        svg { width: "{size}", height: "{size}", view_box: "0 0 24 24", fill: "currentColor",
            path { d: "M21 5H3v2h18V5zm0 4H3v2h18V9zm0 4H3v6h18v-6z" }
        }
    }
}

pub fn volume(size: i32) -> Element {
    rsx! {
        svg { width: "{size}", height: "{size}", view_box: "0 0 24 24", fill: "currentColor",
            path { d: "M3 9v6h4l5 5V4L7 9H3zm13.5 3a4.5 4.5 0 0 0-2.5-4v8a4.5 4.5 0 0 0 2.5-4zM14 3.2v2.1a7 7 0 0 1 0 13.4v2.1a9 9 0 0 0 0-17.6z" }
        }
    }
}