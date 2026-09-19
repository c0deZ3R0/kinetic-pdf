//! What a tool is set to, and where those settings live.
//!
//! A measurement takes its appearance, its metadata and its measurement
//! options from the tool that drew it, rather than from whatever the toolbar
//! happened to be set to. `ToolSettings` is the whole of what a tool carries.
//!
//! That is deliberate: a tool a user defines and names later -- "Concrete
//! slab 200", say, red, 200 mm deep, filed under an item code -- is this same
//! structure with a name on it. Nothing new has to be modelled for it, and
//! anything added here is carried by saved tools for free. The settings in
//! use are kept under the tool they belong to, so picking up the area tool
//! picks up how the area tool was left.
//!
//! Written to a small JSON file beside the page cache, so a tool set up once
//! is still set up tomorrow.

use std::collections::BTreeMap;
use std::path::PathBuf;

use markup_model::markup::{MarkupKind as MeasureKind, Slope, Style};
use serde::{Deserialize, Serialize};

use super::*;

/// Which tool a set of settings belongs to. Drawing tools are keyed here as
/// well as measurements: they don't read their settings from here yet, but a
/// saved tool has to be able to name either.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum ToolKey {
    Measure(MeasureTool),
    Draw(MarkupKind),
}

impl ToolKey {
    /// The name it is stored under. Written into the settings file, so these
    /// strings are not free to change once shipped.
    fn stored(self) -> String {
        match self {
            ToolKey::Measure(tool) => format!("measure.{}", tool.label().to_lowercase()),
            ToolKey::Draw(kind) => format!("draw.{}", kind.label().to_lowercase()),
        }
    }

    /// The tool that draws a measurement of this kind, for editing one that
    /// is already down. Kinds no tool here draws have none.
    pub fn of_measurement(kind: MeasureKind) -> Option<ToolKey> {
        let tool = match kind {
            MeasureKind::Length => MeasureTool::Length,
            MeasureKind::Polylength => MeasureTool::Polylength,
            MeasureKind::Area | MeasureKind::Volume | MeasureKind::Perimeter => MeasureTool::Area,
            MeasureKind::Count => MeasureTool::Count,
            MeasureKind::Angle => MeasureTool::Angle,
            MeasureKind::Radius => MeasureTool::Radius,
            MeasureKind::Diameter => MeasureTool::Diameter,
            _ => return None,
        };
        Some(ToolKey::Measure(tool))
    }

    /// Whether a depth makes sense: an area with a depth is a volume.
    pub fn takes_depth(self) -> bool {
        matches!(self, ToolKey::Measure(MeasureTool::Area))
    }

    /// Whether a slope makes sense: lengths and areas lying on a pitch are
    /// divided by its cosine, and nothing else is.
    pub fn takes_slope(self) -> bool {
        matches!(self, ToolKey::Measure(MeasureTool::Length | MeasureTool::Polylength | MeasureTool::Area))
    }

    /// Whether it fills what it draws.
    pub fn fills(self) -> bool {
        matches!(self, ToolKey::Measure(MeasureTool::Area) | ToolKey::Draw(MarkupKind::Rectangle | MarkupKind::Ellipse))
    }
}

/// What a new measurement is called and filed under before anyone types
/// anything: the part of a tool that a bill of quantities cares about.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(super) struct ToolDefaults {
    /// Goes in the measurement's Name column.
    pub name: String,
    /// Goes in its Description column, which is what a take-off prices by.
    pub description: String,
    /// A bill-of-quantities item code, `A-120`.
    pub item_code: String,
    pub layer: String,
    pub status: String,
}

/// Everything a tool carries. Also what a measurement already drawn can
/// have changed about it, which is the same set: see `tool_panel.rs`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct ToolSettings {
    pub style: Style,
    #[serde(default)]
    pub defaults: ToolDefaults,
    /// Areas only: what turns the area into a volume as it is drawn.
    #[serde(default)]
    pub depth_m: Option<f64>,
    /// Lengths and areas only: the pitch they lie on.
    #[serde(default)]
    pub slope: Option<Slope>,
}

impl ToolSettings {
    /// How a tool starts out: red, thin, and filled if it draws something
    /// with an inside.
    fn new(key: ToolKey) -> ToolSettings {
        let mut style = Style::default();
        if key.fills() {
            style.fill = Some(style.stroke);
            // Outlined solidly, filled faintly: the line is what is measured
            // and has to read against the drawing under it.
            style.fill_opacity = 0.18;
        }
        ToolSettings { style, defaults: ToolDefaults::default(), depth_m: None, slope: None }
    }

    /// Whether it is still how it started, so the file needn't hold it.
    fn untouched(&self, key: ToolKey) -> bool {
        self == &ToolSettings::new(key)
    }

    /// What a measurement already drawn is set to, so the same form that sets
    /// a tool up can change one that is already down.
    pub fn of_markup(markup: &markup_model::Markup) -> ToolSettings {
        ToolSettings {
            style: markup.style.clone(),
            defaults: ToolDefaults {
                name: markup.meta.name.clone(),
                description: markup.meta.label.clone(),
                item_code: markup.meta.item_code.clone().unwrap_or_default(),
                layer: markup.meta.layer.clone().unwrap_or_default(),
                status: markup.meta.status.clone().unwrap_or_default(),
            },
            depth_m: markup.extras.depth_m,
            slope: markup.extras.slope,
        }
    }

    /// Puts these settings on a measurement just drawn, or on one being
    /// changed.
    pub fn apply(&self, markup: &mut markup_model::Markup) {
        markup.style = self.style.clone();
        // A fill only makes sense on a shape with an inside; the stroke
        // colour leads, so a fill follows it unless one was chosen.
        if !matches!(markup.geometry, markup_model::Geometry::Polygon { .. }) {
            markup.style.fill = None;
        }
        let d = &self.defaults;
        markup.meta.name = d.name.clone();
        markup.meta.label = d.description.clone();
        markup.meta.item_code = (!d.item_code.is_empty()).then(|| d.item_code.clone());
        markup.meta.layer = (!d.layer.is_empty()).then(|| d.layer.clone());
        markup.meta.status = (!d.status.is_empty()).then(|| d.status.clone());
        if markup.kind == MeasureKind::Area {
            markup.extras.depth_m = self.depth_m;
        }
        markup.extras.slope = self.slope;
    }
}

/// Every tool's settings, by tool.
#[derive(Default)]
pub(super) struct Tools {
    /// Only the tools that have been changed from how they start are held; a
    /// tool not in here is at its defaults.
    changed: BTreeMap<String, ToolSettings>,
    /// Changed since the last write. Typing a description would otherwise
    /// write the file at every keystroke.
    unsaved: bool,
}

impl Tools {
    pub fn settings(&self, key: ToolKey) -> ToolSettings {
        self.changed.get(&key.stored()).cloned().unwrap_or_else(|| ToolSettings::new(key))
    }

    /// Changes a tool's settings, and writes them out. Saving is a few
    /// hundred bytes, so it happens on the change rather than being tracked.
    pub fn set(&mut self, key: ToolKey, settings: ToolSettings) {
        let stored = key.stored();
        if settings.untouched(key) {
            self.changed.remove(&stored);
        } else {
            self.changed.insert(stored, settings);
        }
        self.unsaved = true;
    }

    /// Puts a tool back to how it started.
    pub fn reset(&mut self, key: ToolKey) {
        self.changed.remove(&key.stored());
        self.unsaved = true;
    }

    /// Writes the settings out if they have changed. Called as the app goes
    /// round, so a run of keystrokes costs one write rather than one each.
    pub fn flush(&mut self) {
        if std::mem::take(&mut self.unsaved) {
            self.save();
        }
    }

    /// Whether a tool has been changed from how it starts.
    pub fn is_changed(&self, key: ToolKey) -> bool {
        self.changed.contains_key(&key.stored())
    }

    pub fn load() -> Tools {
        let Some(path) = settings_path() else { return Tools::default() };
        let Ok(text) = std::fs::read_to_string(path) else { return Tools::default() };
        // Settings that can't be read are settings at their defaults: a file
        // from a newer version, or a half-written one, must not stop the app.
        Tools { changed: serde_json::from_str(&text).unwrap_or_default(), unsaved: false }
    }

    fn save(&self) {
        let Some(path) = settings_path() else { return };
        let Ok(text) = serde_json::to_string_pretty(&self.changed) else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, text);
    }
}

/// Beside the page cache, in the folder the OS gives the app for itself.
fn settings_path() -> Option<PathBuf> {
    crate::cache::default_dir().parent().map(|dir| dir.join("tools.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tool_starts_at_its_defaults_and_comes_back_to_them() {
        let key = ToolKey::Measure(MeasureTool::Area);
        let mut tools = Tools::default();
        assert!(!tools.is_changed(key));
        let mut settings = tools.settings(key);
        // An area starts filled; a length does not.
        assert!(settings.style.fill.is_some());
        assert!(tools.settings(ToolKey::Measure(MeasureTool::Length)).style.fill.is_none());

        settings.depth_m = Some(0.2);
        tools.changed.insert(key.stored(), settings);
        assert!(tools.is_changed(key));
        assert_eq!(tools.settings(key).depth_m, Some(0.2));

        tools.changed.remove(&key.stored());
        assert!(!tools.is_changed(key));
        assert_eq!(tools.settings(key).depth_m, None);
    }

    /// Settings back to their starting values are dropped rather than stored,
    /// so the file holds only what was really changed.
    #[test]
    fn settings_put_back_as_they_were_are_not_kept() {
        let key = ToolKey::Measure(MeasureTool::Length);
        let mut settings = Tools::default().settings(key);
        settings.style.width = 4.0;
        assert!(!settings.untouched(key));
        settings.style.width = ToolSettings::new(key).style.width;
        assert!(settings.untouched(key));
    }

    #[test]
    fn stored_names_are_stable_and_distinct() {
        assert_eq!(ToolKey::Measure(MeasureTool::Area).stored(), "measure.area");
        assert_eq!(ToolKey::Draw(MarkupKind::Pen).stored(), "draw.pen");
        assert_ne!(ToolKey::Measure(MeasureTool::Length).stored(), ToolKey::Draw(MarkupKind::Line).stored());
    }

    #[test]
    fn defaults_reach_the_measurement_they_draw() {
        let key = ToolKey::Measure(MeasureTool::Area);
        let mut settings = ToolSettings::new(key);
        settings.defaults.name = "Slab".to_owned();
        settings.defaults.description = "Concrete slab".to_owned();
        settings.defaults.item_code = "A-120".to_owned();
        settings.depth_m = Some(0.2);
        let mut markup = markup_model::Markup::new(
            0,
            MeasureKind::Area,
            markup_model::Geometry::Polygon { pts: Vec::new(), holes: Vec::new() },
        );
        settings.apply(&mut markup);
        assert_eq!(markup.meta.name, "Slab");
        assert_eq!(markup.meta.label, "Concrete slab");
        assert_eq!(markup.meta.item_code.as_deref(), Some("A-120"));
        assert_eq!(markup.extras.depth_m, Some(0.2));
        assert!(markup.style.fill.is_some(), "an area keeps its fill");
    }

    /// What the panel reads off a measurement is what `apply` put there, so
    /// opening a measurement and changing nothing changes nothing.
    #[test]
    fn settings_read_back_off_a_measurement_are_the_ones_put_on_it() {
        let key = ToolKey::Measure(MeasureTool::Area);
        let mut settings = ToolSettings::new(key);
        settings.defaults.name = "Slab".to_owned();
        settings.defaults.description = "Concrete slab".to_owned();
        settings.defaults.item_code = "A-120".to_owned();
        settings.defaults.layer = "Structure".to_owned();
        settings.depth_m = Some(0.2);
        settings.slope = Some(Slope { rise: 1.0, run: 10.0 });
        settings.style.width = 4.0;

        let mut markup = markup_model::Markup::new(
            0,
            MeasureKind::Area,
            markup_model::Geometry::Polygon { pts: Vec::new(), holes: Vec::new() },
        );
        settings.apply(&mut markup);
        assert_eq!(ToolSettings::of_markup(&markup), settings);
    }

    /// A measurement points back at the tool that draws its kind, so the
    /// panel knows whether to offer it a depth.
    #[test]
    fn a_measurement_knows_which_tool_drew_it() {
        assert_eq!(ToolKey::of_measurement(MeasureKind::Area), Some(ToolKey::Measure(MeasureTool::Area)));
        // A volume is an area with a depth, so the area tool owns it.
        assert_eq!(ToolKey::of_measurement(MeasureKind::Volume), Some(ToolKey::Measure(MeasureTool::Area)));
        assert_eq!(ToolKey::of_measurement(MeasureKind::Diameter), Some(ToolKey::Measure(MeasureTool::Diameter)));
        // Nothing here draws a highlight, so it has no tool settings.
        assert_eq!(ToolKey::of_measurement(MeasureKind::Highlight), None);
    }

    /// An empty default is no default: nothing is written for it.
    #[test]
    fn empty_defaults_leave_the_measurement_alone() {
        let settings = ToolSettings::new(ToolKey::Measure(MeasureTool::Length));
        assert_eq!(settings.defaults, ToolDefaults::default());
        let mut markup =
            markup_model::Markup::new(0, MeasureKind::Length, markup_model::Geometry::Line { a: Default::default(), b: Default::default() });
        settings.apply(&mut markup);
        assert_eq!(markup.meta.item_code, None);
        assert_eq!(markup.meta.layer, None);
        assert!(markup.style.fill.is_none(), "a length has no inside to fill");
    }
}
