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

/// Every measurement tool that draws something kept, in the order the tool
/// row shows them. Calibrating and checking set the page's scale instead, so
/// they carry no settings.
pub(super) const MEASURE_TOOLS: [MeasureTool; 8] = [
    MeasureTool::Length,
    MeasureTool::Polylength,
    MeasureTool::Area,
    MeasureTool::Cutout,
    MeasureTool::Count,
    MeasureTool::Angle,
    MeasureTool::Radius,
    MeasureTool::Diameter,
];

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
    pub fn stored(self) -> String {
        match self {
            ToolKey::Measure(tool) => format!("measure.{}", tool.label().to_lowercase()),
            ToolKey::Draw(kind) => format!("draw.{}", kind.label().to_lowercase()),
        }
    }

    /// Back from the name it is stored under. `None` for a name this version
    /// doesn't know, so a tool saved by a newer one is passed over rather than
    /// taken for the wrong tool.
    pub fn from_stored(name: &str) -> Option<ToolKey> {
        let every = MEASURE_TOOLS
            .into_iter()
            .map(ToolKey::Measure)
            .chain(MarkupKind::TOOLS.into_iter().map(ToolKey::Draw));
        every.into_iter().find(|key| key.stored() == name)
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

    /// What a drawn markup -- a pen stroke, a box, an ellipse, a line, an
    /// arrow -- is set to. It carries fewer of these than a measurement does:
    /// nothing is measured, so there is no depth and no slope, and the
    /// quantity written beside a measurement has nothing to write.
    pub fn of_drawing(markup: &crate::model::Markup) -> ToolSettings {
        let d = &markup.style;
        let mut settings = ToolSettings::new(ToolKey::Draw(markup.kind));
        settings.style.stroke = markup.color;
        settings.style.width = f64::from(markup.width);
        settings.style.width_unit = markup_model::markup::WidthUnit::ScreenPixels;
        settings.style.opacity = d.opacity;
        settings.style.fill = d.fill;
        settings.style.fill_opacity = d.fill_opacity;
        settings.style.pattern = d.pattern;
        settings.style.pattern_colour = d.pattern_colour;
        settings.style.pattern_opacity = d.pattern_opacity;
        settings.style.pattern_size = f64::from(d.pattern_size);
        settings.defaults.name = markup.name.clone();
        settings.defaults.description = markup.comment.clone();
        settings
    }

    /// Puts these settings on a drawn markup: the other way round from
    /// `of_drawing`, and the only way the details panel changes one.
    pub fn apply_to_drawing(&self, markup: &mut crate::model::Markup) {
        let s = &self.style;
        markup.color = s.stroke;
        markup.width = s.width as f32;
        markup.style = crate::model::DrawStyle {
            opacity: s.opacity,
            // Only a shape with an inside takes a fill; a stroke's inside is
            // an accident of where it happens to run.
            fill: s.fill.filter(|_| markup.kind.fills()),
            fill_opacity: s.fill_opacity,
            pattern: s.pattern,
            pattern_colour: s.pattern_colour,
            pattern_opacity: s.pattern_opacity,
            pattern_size: s.pattern_size as f32,
        };
        markup.name = self.defaults.name.clone();
        markup.comment = self.defaults.description.clone();
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

/// Each setting a tool carries, one by one, for editing several things at
/// once: which differ between them, and carrying a change to one setting onto
/// each without touching the rest of it.
macro_rules! each_setting {
    ($($name:ident: $($path:ident).+;)*) => {
        /// Which settings differ between several things picked out together.
        /// The details panel shows those as mixed and leaves them alone on
        /// each thing unless they are changed.
        #[derive(Clone, Copy, Debug, Default, PartialEq)]
        pub(super) struct Mixed {
            $(pub $name: bool,)*
        }

        impl Mixed {
            pub fn of(all: &[ToolSettings]) -> Mixed {
                let Some(first) = all.first() else { return Mixed::default() };
                Mixed { $($name: all.iter().any(|s| s.$($path).+ != first.$($path).+),)* }
            }
        }

        impl ToolSettings {
            /// Puts on these settings whichever of them `after` changed from
            /// `before`, and nothing else.
            pub fn carry(&mut self, before: &ToolSettings, after: &ToolSettings) {
                $(if after.$($path).+ != before.$($path).+ {
                    self.$($path).+ = after.$($path).+.clone();
                })*
            }
        }
    };
}

each_setting! {
    stroke: style.stroke;
    opacity: style.opacity;
    width: style.width;
    width_unit: style.width_unit;
    dash: style.dash;
    fill: style.fill;
    fill_opacity: style.fill_opacity;
    pattern: style.pattern;
    pattern_colour: style.pattern_colour;
    pattern_opacity: style.pattern_opacity;
    pattern_size: style.pattern_size;
    label_font: style.label_font;
    label_colour: style.label_colour;
    label_size: style.label_size;
    name: defaults.name;
    description: defaults.description;
    item_code: defaults.item_code;
    layer: defaults.layer;
    status: defaults.status;
    depth_m: depth_m;
    slope: slope;
}

/// Every setting is in the list above: a new one has to be added there too,
/// or editing several things at once would never change it. Taken apart here
/// in full so that leaving one out doesn't compile.
#[allow(dead_code)]
fn each_setting_is_listed(s: ToolSettings) {
    let ToolSettings { style, defaults, depth_m: _, slope: _ } = s;
    let Style {
        stroke: _,
        fill: _,
        opacity: _,
        fill_opacity: _,
        pattern: _,
        pattern_colour: _,
        pattern_opacity: _,
        pattern_size: _,
        width: _,
        width_unit: _,
        dash: _,
        label_size: _,
        label_colour: _,
        label_font: _,
    } = style;
    let ToolDefaults { name: _, description: _, item_code: _, layer: _, status: _ } = defaults;
}

/// A tool set up once and kept by name: the same settings any tool carries,
/// with a name, a group to file it under, and which tool it draws with.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct SavedTool {
    pub name: String,
    /// What it is filed under in the list. Empty for the ungrouped ones.
    #[serde(default)]
    pub group: String,
    /// Which tool draws it, as `ToolKey::stored`.
    pub key: String,
    pub settings: ToolSettings,
}

impl SavedTool {
    pub fn key(&self) -> Option<ToolKey> {
        ToolKey::from_stored(&self.key)
    }
}

/// The settings file: what each tool is set to, and the tools kept by name.
#[derive(Default, Serialize, Deserialize)]
struct Stored {
    #[serde(default)]
    changed: BTreeMap<String, ToolSettings>,
    #[serde(default)]
    saved: Vec<SavedTool>,
    /// Groups shown rolled up.
    #[serde(default)]
    collapsed: Vec<String>,
}

/// Every tool's settings, by tool, and the tools kept by name.
#[derive(Default)]
pub(super) struct Tools {
    /// Only the tools that have been changed from how they start are held; a
    /// tool not in here is at its defaults.
    changed: BTreeMap<String, ToolSettings>,
    /// Tools kept by name, in the order they are shown.
    saved: Vec<SavedTool>,
    /// Groups shown rolled up.
    collapsed: Vec<String>,
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

    /// Every tool kept by name, grouped: the groups in the order they first
    /// appear, the ungrouped ones last under an empty name.
    pub fn groups(&self) -> Vec<(String, Vec<(usize, &SavedTool)>)> {
        let mut groups: Vec<(String, Vec<(usize, &SavedTool)>)> = Vec::new();
        for (at, tool) in self.saved.iter().enumerate() {
            match groups.iter_mut().find(|(name, _)| *name == tool.group) {
                Some((_, tools)) => tools.push((at, tool)),
                None => groups.push((tool.group.clone(), vec![(at, tool)])),
            }
        }
        // The ungrouped ones read last, under no heading.
        groups.sort_by_key(|(name, _)| name.is_empty());
        groups
    }

    pub fn saved_count(&self) -> usize {
        self.saved.len()
    }

    /// Keeps `settings` by name. A name already used in that group is replaced,
    /// so saving twice over the same name changes it rather than growing a
    /// second one.
    pub fn save_tool(&mut self, name: &str, group: &str, key: ToolKey, mut settings: ToolSettings) {
        let name = name.trim().to_owned();
        // What the tool is called is what its measurements are called: the
        // name given here is the one that shows in the quantities table, so a
        // tool kept as "Concrete slab 200" draws rows called that.
        settings.defaults.name = name.clone();
        let tool = SavedTool { name, group: group.trim().to_owned(), key: key.stored(), settings };
        match self.saved.iter_mut().find(|t| t.name == tool.name && t.group == tool.group) {
            Some(existing) => *existing = tool,
            None => self.saved.push(tool),
        }
        self.unsaved = true;
    }

    pub fn forget_tool(&mut self, at: usize) {
        if at < self.saved.len() {
            self.saved.remove(at);
            self.unsaved = true;
        }
    }

    /// What a tool file holds, written out so a set can be made without this
    /// app: pasted to someone, or to something that writes JSON.
    ///
    /// Kept beside the structures it describes so the two are changed
    /// together, and the example in it is parsed by a test, so a field that
    /// moves here without moving there is caught.
    pub fn schema_json() -> String {
        SCHEMA.to_owned()
    }

    /// The tools kept by name, as JSON to hand to someone else. Only the
    /// tools: what this copy of the app has its own tools set to is nobody
    /// else's business, and neither is which groups are rolled up here.
    pub fn export_json(&self) -> Result<String, String> {
        serde_json::to_string_pretty(&self.saved).map_err(|e| e.to_string())
    }

    /// Takes in tools from a file someone else wrote. A tool whose name and
    /// group match one already here replaces it, so importing the same file
    /// twice leaves one of each rather than two. Gives back how many arrived.
    pub fn import_json(&mut self, text: &str) -> Result<usize, String> {
        // A bare list of tools, or a whole settings file: either is something
        // someone might reasonably hand over.
        let incoming: Vec<SavedTool> = match serde_json::from_str::<Vec<SavedTool>>(text) {
            Ok(tools) => tools,
            Err(list_error) => match serde_json::from_str::<Stored>(text) {
                Ok(stored) if !stored.saved.is_empty() => stored.saved,
                _ => return Err(format!("that file doesn't hold any tools: {list_error}")),
            },
        };
        if incoming.is_empty() {
            return Err("that file doesn't hold any tools".to_owned());
        }
        let taken = incoming.len();
        for tool in incoming {
            match self.saved.iter_mut().find(|t| t.name == tool.name && t.group == tool.group) {
                Some(existing) => *existing = tool,
                None => self.saved.push(tool),
            }
        }
        self.unsaved = true;
        Ok(taken)
    }

    pub fn is_collapsed(&self, group: &str) -> bool {
        self.collapsed.iter().any(|g| g == group)
    }

    pub fn toggle_collapsed(&mut self, group: &str) {
        match self.collapsed.iter().position(|g| g == group) {
            Some(at) => drop(self.collapsed.remove(at)),
            None => self.collapsed.push(group.to_owned()),
        }
        self.unsaved = true;
    }

    pub fn load() -> Tools {
        let Some(path) = settings_path() else { return Tools::default() };
        let Ok(text) = std::fs::read_to_string(path) else { return Tools::default() };
        let stored = read_settings(&text);
        Tools { changed: stored.changed, saved: stored.saved, collapsed: stored.collapsed, unsaved: false }
    }

    fn save(&self) {
        let Some(path) = settings_path() else { return };
        let stored =
            Stored { changed: self.changed.clone(), saved: self.saved.clone(), collapsed: self.collapsed.clone() };
        let Ok(text) = serde_json::to_string_pretty(&stored) else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, text);
    }
}

/// The settings file's contents, whichever shape it is in.
///
/// A file written before tools could be kept by name is the settings map on
/// its own. That shape is tried first, because every field of `Stored` has a
/// default and serde passes over fields it doesn't know: read the other way
/// round, an old file parses as an empty `Stored` and every setting in it is
/// silently thrown away.
///
/// Anything unreadable is read as defaults rather than stopping the app.
fn read_settings(text: &str) -> Stored {
    if let Ok(changed) = serde_json::from_str::<BTreeMap<String, ToolSettings>>(text) {
        return Stored { changed, ..Stored::default() };
    }
    serde_json::from_str(text).unwrap_or_default()
}


/// The shape of a tool file, for the Schema button: a JSON Schema with every
/// field described, the values each may take, and a worked example.
///
/// The example is parsed by a test, so it cannot drift from the structures
/// above without something noticing.
const SCHEMA: &str = r##"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "Kinetic PDF tools",
  "description": "A set of measurement tools, as written by Export and read by Import. A colour is [red, green, blue], each 0 to 1. A length is in points on the page (72 to the inch) unless it says otherwise; a depth is in metres.",
  "type": "array",
  "items": {
    "type": "object",
    "required": ["name", "key", "settings"],
    "additionalProperties": false,
    "properties": {
      "name": {
        "type": "string",
        "description": "What the tool is called. Every measurement it draws is named this, which is what shows in the Name column of the quantities table."
      },
      "group": {
        "type": "string",
        "default": "",
        "description": "The heading it is filed under in the tools list. Leave empty to put it in no group."
      },
      "key": {
        "type": "string",
        "description": "Which tool draws it.",
        "enum": [
          "measure.length",
          "measure.polylength",
          "measure.area",
          "measure.cutout",
          "measure.count",
          "measure.angle",
          "measure.radius",
          "measure.diameter",
          "draw.pen",
          "draw.rectangle",
          "draw.ellipse",
          "draw.line",
          "draw.arrow"
        ]
      },
      "settings": {
        "type": "object",
        "required": ["style"],
        "additionalProperties": false,
        "properties": {
          "style": {
            "type": "object",
            "description": "How it is drawn: three layers, each with its own colour and transparency -- the inside, whatever is ruled over it, and the line round it.",
            "required": ["stroke", "opacity", "width", "width_unit", "dash", "label_size"],
            "additionalProperties": false,
            "properties": {
              "stroke": { "$ref": "#/$defs/colour", "description": "The line's colour." },
              "opacity": { "$ref": "#/$defs/fraction", "description": "The line's own transparency." },
              "width": { "type": "number", "minimum": 0, "description": "How thick the line is, in the unit below." },
              "width_unit": {
                "enum": ["Points", "ScreenPixels"],
                "description": "Points thicken as you zoom in, as printed. ScreenPixels stay the same on screen at any zoom, which is what the app sets."
              },
              "fill": {
                "oneOf": [{ "$ref": "#/$defs/colour" }, { "type": "null" }],
                "default": null,
                "description": "The inside's colour, or null for a shape with nothing in it. Only shapes with an inside are filled: areas, rectangles and ellipses."
              },
              "fill_opacity": { "$ref": "#/$defs/fraction", "default": 1, "description": "The inside's own transparency, apart from the line's." },
              "pattern": {
                "enum": ["Solid", "Diagonal", "Cross", "Horizontal", "Vertical", "Dots"],
                "default": "Solid",
                "description": "How the inside is ruled, over the fill. Written into the PDF as a tiling pattern, so a hatch costs the same however large the shape."
              },
              "pattern_colour": {
                "oneOf": [{ "$ref": "#/$defs/colour" }, { "type": "null" }],
                "default": null,
                "description": "The ruling's own colour. null follows the line's."
              },
              "pattern_opacity": { "$ref": "#/$defs/fraction", "default": 1, "description": "The ruling's own transparency." },
              "pattern_size": { "type": "number", "minimum": 1, "default": 6, "description": "How far apart the ruling is, in points on the page." },
              "dash": {
                "type": "array",
                "items": { "type": "number", "minimum": 0 },
                "default": [],
                "description": "Dash and gap lengths, in the width's unit. Empty for a solid line."
              },
              "label_size": { "type": "number", "minimum": 1, "default": 10, "description": "The quantity's size, in points on the page." },
              "label_colour": {
                "oneOf": [{ "$ref": "#/$defs/colour" }, { "type": "null" }],
                "default": null,
                "description": "The quantity's own colour. null follows the line's."
              },
              "label_font": {
                "enum": ["Sans", "Mono"],
                "default": "Sans",
                "description": "The face the quantity is written in. Only faces every PDF viewer has without the file carrying one."
              }
            }
          },
          "defaults": {
            "type": "object",
            "description": "What every measurement drawn with this tool is called and filed under before anything is typed.",
            "additionalProperties": false,
            "properties": {
              "name": { "type": "string", "description": "Set from the tool's name when it is kept; shows in the Name column." },
              "description": { "type": "string", "description": "Shows in the Description column, which is what a take-off prices by." },
              "item_code": { "type": "string", "description": "A bill-of-quantities item code, such as A-120." },
              "layer": { "type": "string" },
              "status": { "type": "string" }
            }
          },
          "depth_m": {
            "type": ["number", "null"],
            "default": null,
            "description": "Areas only: how deep, in metres. An area with a depth is measured as a volume."
          },
          "slope": {
            "oneOf": [
              {
                "type": "object",
                "required": ["rise", "run"],
                "additionalProperties": false,
                "properties": { "rise": { "type": "number" }, "run": { "type": "number", "exclusiveMinimum": 0 } }
              },
              { "type": "null" }
            ],
            "default": null,
            "description": "Lengths and areas only: the pitch they lie on, rise over run in any one unit. Plan lengths and areas are divided by its cosine."
          }
        }
      }
    }
  },
  "$defs": {
    "colour": {
      "type": "array",
      "items": { "type": "number", "minimum": 0, "maximum": 1 },
      "minItems": 3,
      "maxItems": 3,
      "description": "Red, green and blue, each 0 to 1."
    },
    "fraction": { "type": "number", "minimum": 0, "maximum": 1 }
  },
  "examples": [
    [
      {
        "name": "Concrete slab 200",
        "group": "Concrete",
        "key": "measure.area",
        "settings": {
          "style": {
            "stroke": [0.2, 0.35, 0.8],
            "opacity": 1.0,
            "width": 2.0,
            "width_unit": "ScreenPixels",
            "fill": [0.2, 0.35, 0.8],
            "fill_opacity": 0.15,
            "pattern": "Diagonal",
            "pattern_colour": [0.1, 0.1, 0.1],
            "pattern_opacity": 0.6,
            "pattern_size": 8.0,
            "dash": [],
            "label_size": 10.0,
            "label_colour": null,
            "label_font": "Sans"
          },
          "defaults": {
            "name": "Concrete slab 200",
            "description": "Concrete slab, 200 thick",
            "item_code": "A-120",
            "layer": "Structure",
            "status": ""
          },
          "depth_m": 0.2,
          "slope": null
        }
      }
    ]
  ]
}"##;
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

    /// Of several things, the settings they don't share are the mixed ones;
    /// one alone, or none, has nothing mixed.
    #[test]
    fn settings_that_differ_are_mixed() {
        let area = ToolSettings::new(ToolKey::Measure(MeasureTool::Area));
        let mut slab = area.clone();
        slab.defaults.description = "Slab".to_owned();
        slab.depth_m = Some(0.2);
        let mixed = Mixed::of(&[area.clone(), slab.clone(), area.clone()]);
        assert!(mixed.description && mixed.depth_m);
        assert_eq!(Mixed { description: false, depth_m: false, ..mixed }, Mixed::default(), "and nothing else");
        assert_eq!(Mixed::of(&[slab]), Mixed::default());
        assert_eq!(Mixed::of(&[]), Mixed::default());
    }

    /// Carrying a change onto one of several changes only the settings that
    /// changed, and leaves the ones that thing has of its own.
    #[test]
    fn carrying_a_change_touches_only_what_changed() {
        let before = ToolSettings::new(ToolKey::Measure(MeasureTool::Area));
        let mut after = before.clone();
        after.style.stroke = [0.0, 0.0, 1.0];
        after.defaults.name = "Slab".to_owned();
        after.style.dash = vec![3.0, 1.0];

        let mut own = before.clone();
        own.defaults.description = "Ground floor".to_owned();
        own.style.width = 4.0;
        own.depth_m = Some(0.15);
        own.carry(&before, &after);
        assert_eq!((own.style.stroke, own.defaults.name.as_str(), own.style.dash.as_slice()), ([0.0, 0.0, 1.0], "Slab", &[3.0, 1.0][..]));
        assert_eq!((own.defaults.description.as_str(), own.style.width, own.depth_m), ("Ground floor", 4.0, Some(0.15)), "its own are kept");

        // Nothing changed, nothing carried.
        let mine = own.clone();
        own.carry(&before, &before);
        assert_eq!(own, mine);
    }
}

#[cfg(test)]
mod saved_tests {
    use super::*;

    fn slab() -> ToolSettings {
        let mut settings = ToolSettings::new(ToolKey::Measure(MeasureTool::Area));
        settings.defaults.description = "Concrete slab".to_owned();
        settings.depth_m = Some(0.2);
        settings
    }

    /// Saving over a name in the same group changes that tool rather than
    /// adding a second one of the same name.
    #[test]
    fn a_tool_saved_twice_under_one_name_is_changed_not_doubled() {
        let key = ToolKey::Measure(MeasureTool::Area);
        let mut tools = Tools::default();
        tools.save_tool("Slab", "Concrete", key, slab());
        let mut thicker = slab();
        thicker.depth_m = Some(0.3);
        tools.save_tool("Slab", "Concrete", key, thicker);
        assert_eq!(tools.saved_count(), 1);
        assert_eq!(tools.saved[0].settings.depth_m, Some(0.3));

        // The same name in another group is its own tool.
        tools.save_tool("Slab", "Paving", key, slab());
        assert_eq!(tools.saved_count(), 2);
    }

    /// Grouped in the order the groups first appear, with the ungrouped ones
    /// last so they read as a list rather than under a heading of their own.
    #[test]
    fn tools_gather_into_their_groups_with_the_ungrouped_last() {
        let key = ToolKey::Measure(MeasureTool::Length);
        let mut tools = Tools::default();
        tools.save_tool("Loose", "", key, slab());
        tools.save_tool("Slab", "Concrete", key, slab());
        tools.save_tool("Kerb", "Concrete", key, slab());
        tools.save_tool("Fence", "Site", key, slab());

        let groups = tools.groups();
        let names: Vec<&str> = groups.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["Concrete", "Site", ""]);
        assert_eq!(groups[0].1.len(), 2, "both concrete tools together");
    }

    /// A tool names the tool it draws with, and finds it again.
    #[test]
    fn a_saved_tool_knows_which_tool_draws_it() {
        let key = ToolKey::Measure(MeasureTool::Area);
        let mut tools = Tools::default();
        tools.save_tool("Slab", "Concrete", key, slab());
        assert_eq!(tools.saved[0].key(), Some(key));
        // A tool from a version that knows a tool this one doesn't is passed
        // over rather than taken for the wrong one.
        assert_eq!(ToolKey::from_stored("measure.something-new"), None);
    }

    /// Keeping a tool by name names what it draws: the name reaches the
    /// measurement, and so the Name column of the quantities table.
    #[test]
    fn the_name_a_tool_is_kept_under_names_what_it_draws() {
        let key = ToolKey::Measure(MeasureTool::Area);
        let mut tools = Tools::default();
        tools.save_tool("Concrete slab 200", "Concrete", key, slab());
        assert_eq!(tools.saved[0].settings.defaults.name, "Concrete slab 200");

        // And it is on every measurement that tool draws.
        let mut markup =
            markup_model::Markup::new(0, MeasureKind::Area, markup_model::Geometry::Polygon { pts: Vec::new(), holes: Vec::new() });
        tools.saved[0].settings.apply(&mut markup);
        assert_eq!(markup.meta.name, "Concrete slab 200");
    }

    /// Tools go out as a file and come back the same. Importing twice leaves
    /// one of each rather than two, so a set can be handed round and refreshed.
    #[test]
    fn tools_go_out_to_a_file_and_come_back() {
        let key = ToolKey::Measure(MeasureTool::Area);
        let mut theirs = Tools::default();
        theirs.save_tool("Slab", "Concrete", key, slab());
        theirs.save_tool("Kerb", "Concrete", key, slab());
        let file = theirs.export_json().unwrap();

        let mut mine = Tools::default();
        mine.save_tool("Fence", "Site", key, slab());
        assert_eq!(mine.import_json(&file).unwrap(), 2);
        assert_eq!(mine.saved_count(), 3, "its own tool, and both of theirs");

        // The same file again refreshes them rather than doubling them.
        assert_eq!(mine.import_json(&file).unwrap(), 2);
        assert_eq!(mine.saved_count(), 3);
    }

    /// A whole settings file is something someone might hand over, so the
    /// tools in it are taken; anything else says so rather than half-working.
    #[test]
    fn a_file_with_no_tools_in_it_is_refused() {
        let key = ToolKey::Measure(MeasureTool::Area);
        let mut tools = Tools::default();
        tools.save_tool("Slab", "Concrete", key, slab());
        let whole = serde_json::to_string(&Stored { changed: BTreeMap::new(), saved: tools.saved.clone(), collapsed: Vec::new() }).unwrap();

        let mut mine = Tools::default();
        assert_eq!(mine.import_json(&whole).unwrap(), 1, "the tools out of a whole settings file");
        assert!(mine.import_json("{}").is_err());
        assert!(mine.import_json("not json at all").is_err());
        assert_eq!(mine.saved_count(), 1, "nothing was half-taken");
    }

    /// The schema's worked example is a tool file this app really reads. A
    /// field renamed in the structures without being renamed in the schema
    /// fails here rather than being found by whoever tried to use it.
    #[test]
    fn the_schema_example_is_a_tool_file_that_reads() {
        let schema: serde_json::Value = serde_json::from_str(&Tools::schema_json()).expect("the schema is JSON");
        let example = &schema["examples"][0];
        assert!(example.is_array(), "the example is a list of tools: {example}");

        let mut tools = Tools::default();
        let taken = tools.import_json(&example.to_string()).expect("the example imports");
        assert_eq!(taken, 1);

        // Everything the example says is there is really there.
        let tool = &tools.saved[0];
        assert_eq!(tool.name, "Concrete slab 200");
        assert_eq!(tool.group, "Concrete");
        assert_eq!(tool.key(), Some(ToolKey::Measure(MeasureTool::Area)));
        assert_eq!(tool.settings.depth_m, Some(0.2));
        assert_eq!(tool.settings.defaults.item_code, "A-120");
        assert_eq!(tool.settings.style.pattern, markup_model::FillPattern::Diagonal);
        assert_eq!(tool.settings.style.pattern_size, 8.0);
        assert_eq!(tool.settings.style.label_font, markup_model::LabelFont::Sans);
    }

    /// Every tool the app has is named in the schema, so a file written from
    /// it can reach all of them.
    #[test]
    fn the_schema_names_every_tool() {
        let schema = Tools::schema_json();
        for key in MEASURE_TOOLS.into_iter().map(ToolKey::Measure).chain(MarkupKind::TOOLS.into_iter().map(ToolKey::Draw)) {
            assert!(schema.contains(&format!("\"{}\"", key.stored())), "the schema leaves out {}", key.stored());
        }
    }

    #[test]
    fn a_group_rolls_up_and_back_down() {
        let mut tools = Tools::default();
        assert!(!tools.is_collapsed("Concrete"));
        tools.toggle_collapsed("Concrete");
        assert!(tools.is_collapsed("Concrete"));
        assert!(!tools.is_collapsed("Site"));
        tools.toggle_collapsed("Concrete");
        assert!(!tools.is_collapsed("Concrete"));
    }

    /// A settings file written before tools could be kept by name is still
    /// read, rather than being thrown away as unreadable.
    #[test]
    fn a_settings_file_from_before_saved_tools_still_reads() {
        let key = ToolKey::Measure(MeasureTool::Area);
        let mut changed = BTreeMap::new();
        changed.insert(key.stored(), slab());
        let old = serde_json::to_string(&changed).unwrap();

        let stored = read_settings(&old);
        assert_eq!(stored.changed.get(&key.stored()).unwrap().depth_m, Some(0.2), "the settings in an old file survive");
        assert!(stored.saved.is_empty());

        // And a file in the new shape still reads as itself.
        let mut tools = Tools::default();
        tools.save_tool("Slab", "Concrete", key, slab());
        let new = serde_json::to_string(&Stored { changed, saved: tools.saved.clone(), collapsed: Vec::new() }).unwrap();
        let stored = read_settings(&new);
        assert_eq!(stored.saved.len(), 1, "a new file keeps the tools kept by name");
        assert_eq!(stored.changed.len(), 1, "and the settings beside them");
    }
}
