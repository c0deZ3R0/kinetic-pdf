//! Changing layers, as the commands a panel or a menu hands the session.
//!
//! Each function works out from the session what one request comes to and
//! gives it back as a single `Command`, so it undoes as one step however
//! many markups it touched. Nothing here changes the session itself.

use std::collections::BTreeMap;

use markup_model::{next_z, restacked, LayerId, LayerStack, MarkupId, Restack};

use crate::domain::MeasureMarkup;
use crate::session::{Command, Session};

/// Resolve a drawing's layer, including any preset path or colour changes.
/// Callers can use a cloned stack to include these changes in undo history.
pub fn target_for_new(layers: &mut LayerStack, active: LayerId, preset: &str, colour: Option<[f32; 3]>) -> LayerId {
    let wanted = if preset.trim().is_empty() { active } else { layers.named_path(preset) };
    if let (Some(colour), Some(layer), false) = (colour, layers.get(wanted), preset.trim().is_empty()) {
        if layer.colour.is_none() {
            layers.set_colour(wanted, Some(colour));
        }
    }
    let usable = |id: LayerId| layers.is_visible(id) && !layers.is_locked(id);
    if usable(wanted) { return wanted; }
    layers.back_to_front().iter().rev().copied().find(|&id| usable(id)).unwrap_or(wanted)
}

/// The layers changed by `change`, as a command. `None` if it changed nothing.
pub fn edit(session: &Session, change: impl FnOnce(&mut LayerStack)) -> Option<Command> {
    let mut layers = session.layers().clone();
    change(&mut layers);
    (&layers != session.layers()).then_some(Command::SetLayers(layers))
}

/// A new layer on top, called `name` or near it, and its ID.
pub fn add(session: &Session, name: &str) -> (Command, LayerId) {
    let mut layers = session.layers().clone();
    let id = layers.add(name);
    (Command::SetLayers(layers), id)
}

/// A new layer at the front of `parent`, or of the top level, and its ID.
pub fn add_in(session: &Session, parent: Option<LayerId>, name: &str) -> (Command, LayerId) {
    let mut layers = session.layers().clone();
    let id = layers.add_in(parent, name);
    (Command::SetLayers(layers), id)
}

/// Takes layer `id` out, and refiles what was on it -- keeping its order --
/// in front on the layer it was inside, or the default layer. What was inside
/// it stays where it was, in what it was inside. `None` for the default
/// layer, which stays.
pub fn remove(session: &Session, id: LayerId) -> Option<Command> {
    let mut layers = session.layers().clone();
    let parent = layers.parent_of(id).unwrap_or(LayerId::DEFAULT);
    layers.remove(id)?;
    let mut refiled = vec![Command::SetLayers(layers)];
    let ids: Vec<MarkupId> = session.measures().on_layer(id).map(|m| m.id).collect();
    refiled.extend(refile(session, &ids, parent));
    if session.pins().iter().any(|pin| pin.layer == id) {
        let mut pins = session.pins().to_vec();
        for pin in &mut pins {
            if pin.layer == id { pin.layer = parent; }
        }
        refiled.push(Command::SetPins(pins));
    }
    Some(Command::Batch(refiled))
}

/// Moves markups to layer `to`, in front of what's there, keeping their order
/// among themselves. `None` if all are on it already.
pub fn move_to_layer(session: &Session, ids: &[MarkupId], to: LayerId) -> Option<Command> {
    let moved: Vec<MarkupId> = ids.iter().copied().filter(|&id| session.measures().get(id).is_some_and(|m| m.layer != to)).collect();
    let commands = refile(session, &moved, to);
    (!commands.is_empty()).then_some(Command::Batch(commands))
}

fn refile(session: &Session, ids: &[MarkupId], to: LayerId) -> Vec<Command> {
    let mut moving: Vec<&MeasureMarkup> = ids.iter().filter_map(|&id| session.measures().get(id)).collect();
    session.layers().sort_back_to_front(&mut moving);
    let mates: Vec<&MeasureMarkup> = session.measures().on_layer(to).filter(|m| !ids.contains(&m.id)).collect();
    let base = next_z(&mates);
    moving
        .into_iter()
        .zip(0..)
        .map(|(m, place)| {
            let mut moved = m.clone();
            moved.layer = to;
            moved.extras.z = base + f64::from(place);
            Command::ChangeMeasure(Box::new(moved))
        })
        .collect()
}

/// Moves markups forward or back within their layers. Several picked
/// together keep their order among themselves. `None` if nothing would move.
pub fn restack(session: &Session, ids: &[MarkupId], how: Restack) -> Option<Command> {
    let mut by_layer: BTreeMap<LayerId, Vec<MarkupId>> = BTreeMap::new();
    for &id in ids {
        if let Some(m) = session.measures().get(id) {
            by_layer.entry(m.layer).or_default().push(id);
        }
    }
    let mut commands = Vec::new();
    for (layer, picked) in by_layer {
        let mut mates: Vec<MeasureMarkup> = session.measures().on_layer(layer).cloned().collect();
        let order = |a: &MarkupId, b: &MarkupId| {
            let z = |id: &MarkupId| mates.iter().find(|m| m.id == *id).map_or(0.0, |m| m.extras.z);
            z(a).total_cmp(&z(b)).then(a.cmp(b))
        };
        let mut picked = picked;
        picked.sort_by(order);
        // Each goes in the direction of travel with the one leading it out of
        // the way, so none jumps over another picked with it.
        if matches!(how, Restack::ToBack | Restack::Forward) {
            picked.reverse();
        }
        let mut changed: Vec<MarkupId> = Vec::new();
        for id in picked {
            let refs: Vec<&MeasureMarkup> = mates.iter().collect();
            if let Some(z) = restacked(&refs, id, how) {
                if let Some(m) = mates.iter_mut().find(|m| m.id == id) {
                    m.extras.z = z;
                    changed.push(id);
                }
            }
        }
        commands.extend(changed.into_iter().filter_map(|id| mates.iter().find(|m| m.id == id)).map(|m| Command::ChangeMeasure(Box::new(m.clone()))));
    }
    (!commands.is_empty()).then_some(Command::Batch(commands))
}

#[cfg(test)]
mod tests {
    use super::*;
    use markup_model::{Geometry, MarkupKind, Pt};

    fn drawn(session: &mut Session, layer: LayerId) -> MarkupId {
        let mut m = MeasureMarkup::new(0, MarkupKind::Length, Geometry::Line { a: Pt::new(0.0, 0.0), b: Pt::new(1.0, 0.0) });
        m.layer = layer;
        let id = m.id;
        session.apply(Command::AddMeasure(Box::new(m)));
        id
    }

    fn stacked(session: &Session) -> Vec<MarkupId> {
        session.measures().stacked(0, session.layers()).into_iter().map(|m| m.id).collect()
    }

    #[test]
    fn a_new_markup_goes_in_front_of_its_layer() {
        let mut s = Session::default();
        let ids: Vec<MarkupId> = (0..3).map(|_| drawn(&mut s, LayerId::DEFAULT)).collect();
        assert_eq!(stacked(&s), ids);
    }

    #[test]
    fn restacking_one_or_several_keeps_their_order_and_undoes_as_one_step() {
        let mut s = Session::default();
        let [a, b, c, d] = [(); 4].map(|_| drawn(&mut s, LayerId::DEFAULT));
        s.apply(restack(&s, &[a], Restack::ToFront).unwrap());
        assert_eq!(stacked(&s), [b, c, d, a]);
        s.apply(restack(&s, &[b, c], Restack::ToFront).unwrap());
        assert_eq!(stacked(&s), [d, a, b, c], "picked together, they keep their order");
        s.apply(restack(&s, &[a, d], Restack::ToBack).unwrap());
        assert_eq!(stacked(&s), [d, a, b, c]);
        s.apply(restack(&s, &[b], Restack::Backward).unwrap());
        assert_eq!(stacked(&s), [d, b, a, c]);
        s.apply(restack(&s, &[b], Restack::Forward).unwrap());
        assert_eq!(stacked(&s), [d, a, b, c]);
        assert!(restack(&s, &[c], Restack::ToFront).is_none(), "already in front");
        assert!(s.undo());
        assert_eq!(stacked(&s), [d, b, a, c], "one step back, however many it moved");
    }

    #[test]
    fn moving_to_a_layer_puts_markups_in_front_there_and_layers_set_the_order() {
        let mut s = Session::default();
        let (cmd, notes) = add(&s, "Notes");
        s.apply(cmd);
        let (a, b) = (drawn(&mut s, LayerId::DEFAULT), drawn(&mut s, LayerId::DEFAULT));
        let n = drawn(&mut s, notes);
        assert_eq!(stacked(&s), [a, b, n], "the layer above draws above");
        s.apply(move_to_layer(&s, &[a], notes).unwrap());
        assert_eq!(stacked(&s), [b, n, a]);
        assert!(move_to_layer(&s, &[a, n], notes).is_none());
        s.apply(edit(&s, |l| {
            l.move_to(notes, 0);
        })
        .unwrap());
        assert_eq!(stacked(&s), [n, a, b]);
    }

    #[test]
    fn taking_a_layer_out_refiles_its_markups_and_undoes_together() {
        let mut s = Session::default();
        let (cmd, notes) = add(&s, "Notes");
        s.apply(cmd);
        let (a, n1, n2) = (drawn(&mut s, LayerId::DEFAULT), drawn(&mut s, notes), drawn(&mut s, notes));
        assert!(remove(&s, LayerId::DEFAULT).is_none());
        s.apply(remove(&s, notes).unwrap());
        assert_eq!(stacked(&s), [a, n1, n2], "in front on the default layer, in their order");
        assert!(s.layers().get(notes).is_none());
        assert!(s.undo());
        assert!(s.layers().get(notes).is_some() && s.measures().get(n1).unwrap().layer == notes);
    }

    #[test]
    fn a_layer_inside_another_is_taken_out_into_it_and_dropping_reorders_and_nests() {
        let mut s = Session::default();
        let (command, outer) = add(&s, "Outer");
        s.apply(command);
        let (command, inner) = add_in(&s, Some(outer), "Inner");
        s.apply(command);
        let (o, i) = (drawn(&mut s, outer), drawn(&mut s, inner));
        assert_eq!(stacked(&s), [o, i], "a folder draws under what is inside it");
        s.apply(remove(&s, inner).unwrap());
        assert_eq!(s.measures().get(i).unwrap().layer, outer, "into the folder it was in");
        assert!(s.undo());
        assert_eq!(s.layers().parent_of(inner), Some(outer));
        s.apply(edit(&s, |l| {
            l.place(inner, markup_model::Place::Above(LayerId::DEFAULT));
        })
        .unwrap());
        assert_eq!(s.layers().parent_of(inner), None);
        assert_eq!(s.layers().path(s.layers().by_path("Outer").unwrap()), "Outer");
    }

    #[test]
    fn hidden_and_locked_layers_are_not_drawn_on() {
        let mut s = Session::default();
        let (cmd, notes) = add(&s, "Notes");
        s.apply(cmd);
        s.set_active_layer(notes);
        assert_eq!(s.layer_for_new("", None), notes);
        s.apply(edit(&s, |l| {
            l.set_locked(notes, true);
        })
        .unwrap());
        assert_eq!(s.layer_for_new("", None), LayerId::DEFAULT, "a locked layer takes nothing new");
        assert_eq!(s.layer_for_new("Structure", Some([0.1, 0.2, 0.3])), s.layers().by_path("Structure").unwrap(), "a preset's layer is made");
        assert_eq!(s.layers().get(s.layers().by_path("Structure").unwrap()).unwrap().colour, Some([0.1, 0.2, 0.3]), "and given the tool's colour");
        s.layer_for_new("Structure", Some([0.9, 0.9, 0.9]));
        assert_eq!(s.layers().get(s.layers().by_path("Structure").unwrap()).unwrap().colour, Some([0.1, 0.2, 0.3]), "a colour it has is not overwritten");
    }

    #[test]
    fn layers_are_unsaved_until_saved_and_a_markup_pasted_from_elsewhere_lists_its_layer() {
        let mut s = Session::default();
        assert!(!s.is_dirty());
        let (cmd, notes) = add(&s, "Notes");
        s.apply(cmd);
        assert!(s.is_dirty());
        let changes = s.begin_save("me".into()).unwrap();
        assert_eq!(changes.layers.as_ref().map(|l| l.layers().len()), Some(2));
        s.save_failed();
        assert!(s.undo() && !s.is_dirty(), "undone back to the file as it is");
        let foreign = LayerId::new();
        drawn(&mut s, foreign);
        assert!(s.layers().get(foreign).is_some(), "listed, so it can be seen and moved");
        let _ = notes;
    }
}
