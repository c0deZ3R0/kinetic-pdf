//! Adapter to the existing saved-tool configuration, without a second settings model.
use super::*;
use super::super::tools::{ToolKey, ToolSettings};

impl App {
    pub(super) fn control_tools(&self) -> api::Data {
        let available = actions::Action::catalog().into_iter().filter_map(|action| {
            let key = match action {
                actions::Action::Highlighter => ToolKey::Highlight, actions::Action::Pin => ToolKey::Pin,
                actions::Action::Text(arrow) => ToolKey::Text { arrow }, actions::Action::Draw(kind) => ToolKey::Draw(kind),
                actions::Action::Measure(tool) => ToolKey::Measure(tool), _ => return None,
            };
            ToolKey::from_stored(&key.stored())?;
            Some(api::Tool { name: action.label(), group: String::new(), kind: key.stored(), settings: serde_json::to_value(self.tools.settings(key)).unwrap() })
        }).collect();
        let saved = (0..self.tools.saved_count()).filter_map(|at| self.tools.saved_tool(at)).map(|tool| api::Tool {
            name: tool.name.clone(), group: tool.group.clone(), kind: tool.key.clone(), settings: serde_json::to_value(&tool.settings).unwrap(),
        }).collect();
        api::Data::Tools { saved, available }
    }

    pub(super) fn control_configure_tool(&mut self, name: String, group: String, kind: String, patch: Option<serde_json::Value>, replace: bool) -> std::result::Result<(), Error> {
        checked_name(&name)?;
        if group.len() > 256 { return Err(invalid("Tool group is too long")); }
        let key = ToolKey::from_stored(&kind).ok_or_else(|| invalid("Unknown or non-storable tool kind; list tools first"))?;
        let existing = (0..self.tools.saved_count()).find(|&at| self.tools.saved_tool(at).is_some_and(|t| t.name == name.trim() && t.group == group.trim()));
        if existing.is_some() && !replace { return Err(Error::new(ErrorCode::NeedsConfirmation, "A tool with that name/group exists; set replace explicitly to update it")); }
        let initial = existing.and_then(|at| self.tools.saved_tool(at)).filter(|t| t.key == kind).map_or_else(|| ToolSettings::new(key), |t| t.settings.clone());
        let mut value = serde_json::to_value(initial).map_err(|e| invalid(e.to_string()))?;
        if let Some(patch) = patch { merge_known(&mut value, patch)?; }
        let settings: ToolSettings = serde_json::from_value(value).map_err(|e| invalid(e.to_string()))?;
        validate_settings(&settings)?;
        self.tools.save_tool(&name, &group, key, settings);
        Ok(())
    }

    pub(super) fn control_select_tool(&mut self, kind: &str) -> std::result::Result<(), Error> {
        let key = ToolKey::from_stored(kind).ok_or_else(|| invalid("Unknown tool kind"))?;
        match key {
            ToolKey::Pin => self.take_up_pin(), ToolKey::Highlight => self.take_up_highlighter(),
            ToolKey::Measure(tool) => self.set_measure_tool(Some(tool)), ToolKey::Draw(kind) => self.take_up_drawing(kind),
            ToolKey::Text { arrow } => self.take_up_text(arrow),
        }
        Ok(())
    }

    pub(super) fn control_select_saved(&mut self, name: &str, group: &str) -> std::result::Result<(), Error> {
        let at = (0..self.tools.saved_count()).find(|&at| self.tools.saved_tool(at).is_some_and(|t| t.name == name && t.group == group)).ok_or_else(|| Error::new(ErrorCode::NotFound, "Saved tool name/group not found"))?;
        if self.tools.saved_tool(at).and_then(|t| t.key()).is_none() { return Err(Error::new(ErrorCode::Unsupported, "Saved tool kind is unsupported")); }
        self.take_up_saved(at);
        Ok(())
    }
}

fn merge_known(base: &mut serde_json::Value, patch: serde_json::Value) -> std::result::Result<(), Error> {
    if let (Some(base), Some(patch)) = (base.as_object_mut(), patch.as_object()) {
        for (key, value) in patch {
            let field = base.get_mut(key).ok_or_else(|| invalid(format!("Unknown tool setting: {key}")))?;
            merge_known(field, value.clone())?;
        }
    } else { *base = patch; }
    Ok(())
}

fn validate_settings(s: &ToolSettings) -> std::result::Result<(), Error> {
    let style = &s.style;
    let color = |rgb: [f32; 3]| rgb.into_iter().all(|c| c.is_finite() && (0.0..=1.0).contains(&c));
    if !color(style.stroke) || style.fill.is_some_and(|v| !color(v)) || style.pattern_colour.is_some_and(|v| !color(v)) || style.label_colour.is_some_and(|v| !color(v)) || !color(s.text.format.colour) || s.defaults.layer_colour.is_some_and(|v| !color(v)) { return Err(invalid("Colours must be RGB values between 0 and 1")); }
    for opacity in [style.opacity, style.fill_opacity, style.pattern_opacity] { if !opacity.is_finite() || !(0.0..=1.0).contains(&opacity) { return Err(invalid("Opacity must be between 0 and 1")); } }
    if !style.width.is_finite() || !(0.0..=1000.0).contains(&style.width) || !(0.1..=1000.0).contains(&style.label_size) || !(0.1..=10000.0).contains(&style.pattern_size) || !(0.1..=1000.0).contains(&s.text.format.size) || !(0.0..=1000.0).contains(&s.text.padding) || style.dash.len() > 32 || style.dash.iter().any(|d| !d.is_finite() || *d <= 0.0 || *d > 10000.0) || s.depth_m.is_some_and(|d| !d.is_finite() || !(0.0..=100000.0).contains(&d)) { return Err(invalid("Tool dimensions are outside supported ranges")); }
    Ok(())
}
