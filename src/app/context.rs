//! What a right-click offers, and what it does.
//!
//! A right-click is answered by whatever it landed on, not by the page: the
//! target says what can be done to it, and the menu is those things. A target
//! with nothing to offer -- the bare sheet, for now -- shows no menu at all
//! rather than an empty one.
//!
//! Adding to the menu means adding to `Target::items`, and adding a new kind
//! of thing to right-click on means one more variant and one more arm. The
//! menu itself, and acting on what was chosen, stay as they are.

use super::tools::{ToolKey, ToolSettings};
use super::*;

/// What a right-click landed on.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Target {
    /// A measurement: a length, an area, a count and the rest.
    Measurement(MarkupId),
    /// Something drawn rather than measured: a pen stroke, a box, an arrow.
    Drawing(u64),
    /// The sheet itself, with nothing on it under the pointer.
    Page,
    /// A whole sheet, right-clicked in the sheet view. What the menu offers
    /// is about the sheets picked out, of which this is one; `at` is where a
    /// paste or a blank would go, and `can_paste` whether anything is waiting
    /// to be pasted, since an entry that can do nothing is worse than none.
    Sheet { at: usize, can_paste: bool },
}

/// What a menu entry does when it is chosen.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Action {
    /// Keep it as a tool, named in the panel.
    AddToTools(Target),
    /// Draw the next one the way this one is drawn.
    MakeItTheTool(MarkupId),
    Delete(Target),
    /// Something done to the sheets picked out in the sheet view; see
    /// `arrange.rs`, which is where it is acted on.
    Sheet(SheetAction),
}

/// One entry of the menu.
pub(super) struct Item {
    pub label: &'static str,
    pub action: Action,
    /// Drawn under a rule, to hold it apart from what is above.
    pub apart: bool,
}

impl Target {
    /// What this one offers. An empty list means no menu: right-clicking bare
    /// paper should do nothing rather than open something with nothing in it.
    pub fn items(self) -> Vec<Item> {
        match self {
            Target::Measurement(id) => vec![
                Item { label: "Add to tools…", action: Action::AddToTools(self), apart: false },
                Item { label: "Draw the next one like this", action: Action::MakeItTheTool(id), apart: false },
                Item { label: "Delete", action: Action::Delete(self), apart: true },
            ],
            Target::Drawing(_) => vec![
                Item { label: "Add to tools…", action: Action::AddToTools(self), apart: false },
                Item { label: "Delete", action: Action::Delete(self), apart: true },
            ],
            Target::Page => Vec::new(),
            Target::Sheet { at, can_paste } => {
                let mut items = vec![
                    Item { label: "Cut", action: Action::Sheet(SheetAction::Cut), apart: false },
                    Item { label: "Copy", action: Action::Sheet(SheetAction::Copy), apart: false },
                ];
                if can_paste {
                    items.push(Item { label: "Paste after this sheet", action: Action::Sheet(SheetAction::Paste(Some(at + 1))), apart: false });
                }
                items.push(Item { label: "Duplicate", action: Action::Sheet(SheetAction::Duplicate), apart: false });
                items.push(Item { label: "Rotate 90° clockwise", action: Action::Sheet(SheetAction::Rotate(1)), apart: true });
                items.push(Item { label: "Rotate 90° anticlockwise", action: Action::Sheet(SheetAction::Rotate(-1)), apart: false });
                items.push(Item { label: "Insert a blank sheet after", action: Action::Sheet(SheetAction::InsertBlank(at)), apart: true });
                items.push(Item { label: "Delete", action: Action::Sheet(SheetAction::Delete), apart: true });
                items
            }
        }
    }
}

impl App {
    /// Does what was chosen from a right-click. Kept apart from drawing the
    /// pages, which borrows the document.
    pub(super) fn act_on_context(&mut self, action: Action) {
        match action {
            Action::AddToTools(target) => self.add_to_tools(target),
            Action::MakeItTheTool(id) => {
                let taken = self.doc.as_ref().and_then(|d| d.session.measures().get(id)).and_then(|m| {
                    let key = ToolKey::of_measurement(m.kind)?;
                    Some((key, ToolSettings::of_markup(m)))
                });
                match taken {
                    Some((key, settings)) => {
                        self.tools.set(key, settings);
                        self.toast("The next one is drawn like this one".to_owned());
                    }
                    None => self.toast("Nothing here draws that kind of measurement".to_owned()),
                }
            }
            Action::Delete(Target::Measurement(id)) => {
                if let Some(doc) = self.doc.as_mut() {
                    doc.session.apply(crate::session::Command::RemoveMeasure(id));
                }
                if self.active_measure == Some(id) {
                    self.active_measure = None;
                    self.active_vertex = None;
                }
            }
            Action::Delete(Target::Drawing(uid)) => {
                if let Some(doc) = self.doc.as_mut() {
                    doc.session.apply(crate::session::Command::Remove(uid));
                }
                if self.active == Some(uid) {
                    self.active = None;
                }
            }
            Action::Delete(Target::Page | Target::Sheet { .. }) => {}
            Action::Sheet(action) => self.act_on_sheets(action),
        }
    }

    /// Takes what was right-clicked to the tools list, picked out and with its
    /// name filled in, so all that is left is to say which group it goes in.
    fn add_to_tools(&mut self, target: Target) {
        let named = match target {
            Target::Measurement(id) => {
                self.active_measure = Some(id);
                self.doc
                    .as_ref()
                    .and_then(|d| d.session.measures().get(id))
                    .map(|m| if m.meta.name.is_empty() { m.meta.label.clone() } else { m.meta.name.clone() })
            }
            // A drawing carries no name of its own, so its tool is offered
            // under what kind of thing it is.
            Target::Drawing(uid) => {
                self.active = Some(uid);
                let markup = self.doc.as_ref().and_then(|d| d.session.markup(uid));
                match markup {
                    Some(entry) => {
                        let key = ToolKey::Draw(entry.markup.kind);
                        let mut settings = self.tools.settings(key);
                        settings.style.stroke = entry.markup.color;
                        settings.style.width = f64::from(entry.markup.width);
                        self.tools.set(key, settings);
                        self.tool = Some(entry.markup.kind);
                        self.measure_tool = None;
                        Some(entry.markup.kind.label().to_owned())
                    }
                    None => None,
                }
            }
            Target::Page | Target::Sheet { .. } => return,
        };
        if self.tool_save.0.trim().is_empty() {
            self.tool_save.0 = named.unwrap_or_default();
        }
        self.tool_panel_open = true;
        self.tool_tab = tool_panel::Tab::Tools;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bare paper offers nothing, so no menu opens on it.
    #[test]
    fn the_page_itself_offers_nothing() {
        assert!(Target::Page.items().is_empty());
    }

    /// Everything that can be drawn on can be kept as a tool.
    #[test]
    fn anything_drawn_can_be_added_to_the_tools() {
        for target in [Target::Measurement(MarkupId::new()), Target::Drawing(1)] {
            let items = target.items();
            assert!(!items.is_empty());
            assert!(
                items.iter().any(|i| matches!(i.action, Action::AddToTools(_))),
                "nothing here keeps it as a tool",
            );
            assert!(items.iter().any(|i| matches!(i.action, Action::Delete(_))));
        }
    }

    /// Only a measurement can set what the next measurement looks like.
    #[test]
    fn only_a_measurement_sets_what_the_next_one_looks_like() {
        let measured = Target::Measurement(MarkupId::new()).items();
        assert!(measured.iter().any(|i| matches!(i.action, Action::MakeItTheTool(_))));
        let drawn = Target::Drawing(1).items();
        assert!(!drawn.iter().any(|i| matches!(i.action, Action::MakeItTheTool(_))));
    }
}
