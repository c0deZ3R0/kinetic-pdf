//! How the user likes the app to behave, remembered between runs: for now
//! how fast the mouse wheel scrolls and zooms the document.
//!
//! Kept in a small JSON file beside the author name, in the folder the OS
//! gives the app for itself. Every field has a default, so a file written by
//! an older version -- or a newer one with settings this one doesn't know --
//! still reads, and one that won't read at all is taken as the defaults
//! rather than stopping the app.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// The slowest and fastest the scroll and zoom sliders go.
pub(super) const SPEEDS: std::ops::RangeInclusive<f32> = 0.25..=10.0;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Prefs {
    /// Multiplier for wheel scrolling in the document view.
    pub scroll_speed: f32,
    /// Multiplier for Ctrl-wheel and pinch zoom steps.
    pub zoom_speed: f32,
}

impl Default for Prefs {
    fn default() -> Prefs {
        Prefs { scroll_speed: 1.0, zoom_speed: 1.0 }
    }
}

impl Prefs {
    pub fn load() -> Prefs {
        prefs_file().and_then(|path| std::fs::read_to_string(path).ok()).map_or_else(Prefs::default, |text| Prefs::read(&text))
    }

    pub fn save(&self) {
        let Some(path) = prefs_file() else { return };
        let Ok(text) = serde_json::to_string_pretty(self) else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, text);
    }

    /// The file's contents, with anything unreadable or out of range put
    /// back within reach of the sliders.
    fn read(text: &str) -> Prefs {
        let read: Prefs = serde_json::from_str(text).unwrap_or_default();
        let speed = |value: f32, default: f32| if value.is_finite() { value.clamp(*SPEEDS.start(), *SPEEDS.end()) } else { default };
        let defaults = Prefs::default();
        Prefs { scroll_speed: speed(read.scroll_speed, defaults.scroll_speed), zoom_speed: speed(read.zoom_speed, defaults.zoom_speed) }
    }
}

/// Beside the author name: see `notes::author_file`.
fn prefs_file() -> Option<PathBuf> {
    super::notes::author_file().and_then(|author| author.parent().map(|dir| dir.join("settings.json")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speeds_go_out_and_come_back() {
        let prefs = Prefs { scroll_speed: 2.5, zoom_speed: 0.5 };
        assert_eq!(Prefs::read(&serde_json::to_string(&prefs).unwrap()), prefs);
    }

    /// A file missing a setting, or with one this version doesn't know, still
    /// gives what it does hold.
    #[test]
    fn a_file_with_more_or_fewer_settings_still_reads() {
        assert_eq!(Prefs::read(r#"{ "scroll_speed": 3.0 }"#), Prefs { scroll_speed: 3.0, zoom_speed: 1.0 });
        assert_eq!(Prefs::read(r#"{ "zoom_speed": 2.0, "something_new": true }"#), Prefs { scroll_speed: 1.0, zoom_speed: 2.0 });
    }

    #[test]
    fn an_unreadable_or_out_of_range_file_is_brought_back_within_reach() {
        assert_eq!(Prefs::read("not json"), Prefs::default());
        assert_eq!(Prefs::read(r#"{ "scroll_speed": 500.0, "zoom_speed": 0.0 }"#), Prefs { scroll_speed: 10.0, zoom_speed: 0.25 });
    }
}
