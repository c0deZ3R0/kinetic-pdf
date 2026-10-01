//! How the user likes the app to behave, remembered between runs: how fast
//! the mouse wheel scrolls and zooms the document, and which version last
//! ran, so the first start of a new one can say what changed (whats_new.rs).
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
pub(super) const RECENT_LIMIT: usize = 10;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum UnitSystem {
    #[default]
    Metric,
    Imperial,
}

impl UnitSystem {
    pub fn display(self) -> markup_model::units::DisplayUnits {
        match self {
            Self::Metric => markup_model::units::DisplayUnits::METRIC,
            Self::Imperial => markup_model::units::DisplayUnits::IMPERIAL,
        }
    }
    pub fn precision(self) -> markup_model::units::Precision {
        match self {
            Self::Metric => markup_model::units::Precision::Decimals(2),
            Self::Imperial => markup_model::units::Precision::Fraction(16),
        }
    }
    pub fn paper_factor(self) -> f32 {
        match self {
            Self::Metric => 25.4 / 72.0,
            Self::Imperial => 1.0 / 72.0,
        }
    }
    pub fn paper_suffix(self) -> &'static str {
        match self {
            Self::Metric => "mm",
            Self::Imperial => "in",
        }
    }
    pub fn paper_size(self, points: [f32; 2]) -> String {
        let [w, h] = points.map(|p| p * self.paper_factor());
        match self {
            Self::Metric => format!("{w:.1} × {h:.1} mm"),
            Self::Imperial => format!("{w:.2} × {h:.2} in"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Prefs {
    pub units: UnitSystem,
    pub recent_files: Vec<PathBuf>,
    /// Multiplier for wheel scrolling in the document view.
    pub scroll_speed: f32,
    /// Multiplier for Ctrl-wheel and pinch zoom steps.
    pub zoom_speed: f32,
    /// The version that last ran, as `0.10.0`. `None` in a file written
    /// before versions were kept here.
    pub seen_version: Option<String>,
}

impl Default for Prefs {
    fn default() -> Prefs {
        Prefs {
            units: UnitSystem::Metric,
            recent_files: Vec::new(),
            scroll_speed: 1.0,
            zoom_speed: 1.0,
            seen_version: None,
        }
    }
}

impl Prefs {
    pub fn remember_recent(&mut self, path: PathBuf) {
        self.recent_files
            .retain(|existing| !same_path(existing, &path));
        self.recent_files.insert(0, path);
        self.recent_files.truncate(RECENT_LIMIT);
    }
    pub fn load() -> Prefs {
        prefs_file()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .map_or_else(Prefs::default, |text| Prefs::read(&text))
    }

    pub fn save(&self) {
        let Some(path) = prefs_file() else { return };
        let Ok(text) = serde_json::to_string_pretty(self) else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, text);
    }

    /// The file's contents, with anything unreadable or out of range put
    /// back within reach of the sliders.
    fn read(text: &str) -> Prefs {
        let read: Prefs = serde_json::from_str(text).unwrap_or_default();
        let speed = |value: f32, default: f32| {
            if value.is_finite() {
                value.clamp(*SPEEDS.start(), *SPEEDS.end())
            } else {
                default
            }
        };
        let defaults = Prefs::default();
        let mut prefs = Prefs {
            units: read.units,
            recent_files: Vec::new(),
            scroll_speed: speed(read.scroll_speed, defaults.scroll_speed),
            zoom_speed: speed(read.zoom_speed, defaults.zoom_speed),
            seen_version: read.seen_version,
        };
        for path in read
            .recent_files
            .into_iter()
            .rev()
            .filter(|p| p.is_absolute())
        {
            prefs.remember_recent(path);
        }
        prefs
    }
}

fn same_path(a: &std::path::Path, b: &std::path::Path) -> bool {
    #[cfg(windows)]
    {
        a.as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(&b.as_os_str().to_string_lossy())
    }
    #[cfg(not(windows))]
    {
        a == b
    }
}

/// Beside the author name: see `notes::author_file`.
fn prefs_file() -> Option<PathBuf> {
    super::notes::author_file()
        .and_then(|author| author.parent().map(|dir| dir.join("settings.json")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_files_keep_order_deduplicate_and_survive_a_restart() {
        let mut prefs = Prefs {
            units: UnitSystem::Imperial,
            zoom_speed: 2.0,
            ..Prefs::default()
        };
        let directory = std::env::temp_dir();
        for i in 0..15 {
            prefs.remember_recent(directory.join(format!("drawing-{i}.pdf")));
        }
        let reopened = directory.join("drawing-8.pdf");
        prefs.remember_recent(reopened.clone());
        assert_eq!(prefs.recent_files.len(), RECENT_LIMIT);
        assert_eq!(prefs.recent_files[0], reopened);
        assert_eq!(prefs.recent_files[1], directory.join("drawing-14.pdf"));
        assert_eq!(
            prefs
                .recent_files
                .iter()
                .filter(|p| **p == reopened)
                .count(),
            1
        );
        assert_eq!(Prefs::read(&serde_json::to_string(&prefs).unwrap()), prefs);
    }

    #[cfg(windows)]
    #[test]
    fn recent_paths_are_case_insensitive_on_windows() {
        let mut prefs = Prefs::default();
        prefs.remember_recent(PathBuf::from(r"C:\Drawings\Plan.pdf"));
        prefs.remember_recent(PathBuf::from(r"c:\drawings\PLAN.pdf"));
        assert_eq!(
            prefs.recent_files,
            vec![PathBuf::from(r"c:\drawings\PLAN.pdf")]
        );
    }

    #[test]
    fn page_dimensions_convert_without_changing_the_page() {
        let letter = [612., 792.];
        assert_eq!(UnitSystem::Imperial.paper_size(letter), "8.50 × 11.00 in");
        assert_eq!(UnitSystem::Metric.paper_size(letter), "215.9 × 279.4 mm");
    }

    #[test]
    fn speeds_go_out_and_come_back() {
        let prefs = Prefs {
            units: UnitSystem::Imperial,
            recent_files: Vec::new(),
            scroll_speed: 2.5,
            zoom_speed: 0.5,
            seen_version: Some("0.10.0".to_owned()),
        };
        assert_eq!(Prefs::read(&serde_json::to_string(&prefs).unwrap()), prefs);
    }

    /// A file missing a setting, or with one this version doesn't know, still
    /// gives what it does hold.
    #[test]
    fn a_file_with_more_or_fewer_settings_still_reads() {
        assert_eq!(
            Prefs::read(r#"{ "scroll_speed": 3.0 }"#),
            Prefs {
                scroll_speed: 3.0,
                ..Prefs::default()
            }
        );
        assert_eq!(
            Prefs::read(r#"{ "zoom_speed": 2.0, "something_new": true }"#),
            Prefs {
                zoom_speed: 2.0,
                ..Prefs::default()
            }
        );
    }

    #[test]
    fn an_unreadable_or_out_of_range_file_is_brought_back_within_reach() {
        assert_eq!(Prefs::read("not json"), Prefs::default());
        assert_eq!(
            Prefs::read(r#"{ "scroll_speed": 500.0, "zoom_speed": 0.0 }"#),
            Prefs {
                scroll_speed: 10.0,
                zoom_speed: 0.25,
                ..Prefs::default()
            }
        );
    }
}
