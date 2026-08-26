//! Theme plumbing: applies the persisted theme to the DOM (`<html data-theme>`)
//! and mirrors the few tokens Rust code needs (placeholder art colors).
//!
//! The source of truth for palettes is section 1 of `assets/main.css`. The
//! sync tests below fail the build if the mirrored constants drift from the
//! stylesheet.

use crate::settings::ThemeName;
use dioxus::prelude::ReadableExt;

// ── Mirrored tokens (deep-blue defaults; keep in sync — see tests below) ─────
pub const BG: &str = "#0a0e1a";
pub const SURFACE: &str = "#161c30";
pub const CARD: &str = "#1a2136";
pub const BLUE: &str = "#3b82f6";
pub const BLUE_2: &str = "#60a5fa";
pub const CYAN: &str = "#22d3ee";
pub const TEXT: &str = "#f1f5fb";
pub const MUTED: &str = "#9aa6c3";
pub const RADIUS: &str = "10px";
pub const SIDEBAR_WIDTH: &str = "240px";
pub const TOPBAR_HEIGHT: &str = "56px";
pub const PLAYER_HEIGHT: &str = "92px";

/// Push a theme onto the DOM. Pure CSS variable swap — instant repaint, no
/// Dioxus re-render storm.
pub fn apply_theme(theme: ThemeName) {
    let js = format!(
        "document.documentElement.setAttribute('data-theme', '{}');",
        theme.attr_value()
    );
    let _ = dioxus::document::eval(&js);
}

/// Apply whatever `settings.json` persisted. Called once at app mount.
pub fn apply_persisted_theme() {
    apply_theme(crate::state::SETTINGS.peek().theme);
}

/// User-facing switch (Settings page, Phase 3): update the global signal,
/// persist asynchronously, and repaint immediately.
pub fn set_theme(theme: ThemeName) {
    crate::state::SETTINGS.write().theme = theme;
    let snapshot = *crate::state::SETTINGS.peek();
    dioxus::prelude::spawn(async move {
        if let Err(err) = snapshot.save() {
            tracing::warn!("settings: failed to save theme ({err})");
        }
    });
    apply_theme(theme);
}

/// Allowed, whitelisted artwork hosts rendered by `<img>`.
pub fn is_allowed_media_url(url: &str) -> bool {
    crate::adblock::extract_host(url)
        .map(|host| host.ends_with("scdn.co") || host.ends_with("spotifycdn.com"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;

    /// The whole design system, embedded for drift checks.
    const CSS: &str = include_str!("../../assets/main.css");

    #[test]
    fn mirrored_constants_exist_in_the_stylesheet() {
        // Every mirrored constant must appear as a literal in the default
        // (deep-blue) palette block of main.css.
        for hex in [BG, SURFACE, CARD, BLUE, BLUE_2, CYAN, TEXT, MUTED] {
            assert!(
                CSS.contains(hex),
                "token {hex} drifted out of assets/main.css"
            );
        }
    }

    #[test]
    fn every_theme_variant_has_a_css_hook() {
        // Alternate themes need an explicit override block keyed off the
        // data-theme attribute value; the default theme rides on :root.
        for theme in [ThemeName::Onyx] {
            let needle = format!(":root[data-theme=\"{}\"]", theme.attr_value());
            assert!(
                CSS.contains(&needle),
                "main.css is missing an override block for {needle}"
            );
        }
    }

    #[test]
    fn settings_default_theme_is_represented() {
        // If someone changes the default theme in settings.rs, this forces a
        // conscious update of the stylesheet contract too.
        assert_eq!(Settings::default().theme, ThemeName::DeepBlue);
        assert_eq!(ThemeName::DeepBlue.attr_value(), "deep-blue");
        assert_eq!(ThemeName::Onyx.attr_value(), "onyx");
    }

    #[test]
    fn layout_metrics_match_the_css() {
        assert!(
            CSS.contains(&format!("--sidebar-width: {SIDEBAR_WIDTH}")),
            "--sidebar-width drifted"
        );
        assert!(
            CSS.contains(&format!("--topbar-height: {TOPBAR_HEIGHT}")),
            "--topbar-height drifted"
        );
        assert!(
            CSS.contains(&format!("--player-height: {PLAYER_HEIGHT}")),
            "--player-height drifted"
        );
    }

    #[test]
    fn app_shell_grid_wires_every_shell_zone() {
        // Phase-2 contract: five zones, including the toggleable np column.
        for needle in [
            ".top-bar { grid-area: top; }",
            ".side-nav { grid-area: sidenav; }",
            ".main-content { grid-area: main;",
            ".now-playing-col { grid-area: np;",
            ".player-bar { grid-area: player; }",
            ".bottom-nav { grid-area: nav; }",
            "\"sidenav main   np\"",
        ] {
            assert!(CSS.contains(needle), "shell grid lost `{needle}`");
        }
    }

    #[test]
    fn every_css_custom_property_in_use_is_defined() {
        // Lint the whole stylesheet: any `var(--name)` must have a matching
        // `--name:` definition somewhere (catches typos introduced when
        // renaming tokens across sections).
        let mut defined = std::collections::HashSet::new();
        for line in CSS.lines() {
            let trimmed = line.trim_start();
            if let Some(rest) = trimmed.strip_prefix("--") {
                if let Some((name, _)) = rest.split_once(':') {
                    defined.insert(format!("--{}", name.trim()));
                }
            }
        }

        let mut missing: Vec<String> = Vec::new();
        for (lineno, line) in CSS.lines().enumerate() {
            let mut cursor = 0usize;
            while let Some(rel) = line[cursor..].find("var(--") {
                let abs = cursor + rel;
                let tail = &line[abs..];
                let after_prefix = match tail.strip_prefix("var(--") {
                    Some(t) => t,
                    None => break,
                };
                let end = after_prefix
                    .find([')', ','])
                    .unwrap_or(after_prefix.len());
                // A `var(--name, fallback)` binding (e.g. inline-bound shell
                // variables like --np-width) is self-sufficient; only bare
                // references must resolve to a stylesheet definition.
                if after_prefix[end..].starts_with(',') {
                    cursor = abs + 6 + end;
                    continue;
                }
                let name = format!("--{}", after_prefix[..end].trim());
                if !defined.contains(&name) {
                    missing.push(format!("line {}: {name}", lineno + 1));
                }
                cursor = abs + 6 + end;
            }
        }
        assert!(
            missing.is_empty(),
            "undefined CSS custom properties referenced: {missing:?}"
        );
    }
}