//! Named layers, nested, and what stacks above what.
//!
//! Two levels, kept apart. Layers form a tree: a layer can sit inside another,
//! which then works as a folder for it. Among the layers with one parent the
//! order is back to front, and a layer is drawn under the layers inside it.
//! Inside a layer, each markup has a `z` -- its place among the others there,
//! larger in front -- kept as a real number so moving one markup in front of
//! another changes that one markup only. A markup's place on the page is its
//! layer's place in the tree, then its `z`.
//!
//! Hiding or locking a layer hides or locks everything inside it as well; the
//! layers inside keep their own settings, so showing the folder again brings
//! back exactly what was shown before.
//!
//! A layer is known by a `LayerId` that never changes, so renaming one, or
//! two layers coming to share a name, touches no markup. The default layer,
//! `LayerId::DEFAULT`, is always there, at the top level.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::id::{LayerId, MarkupId};
use crate::markup::Markup;

/// What the default layer is called until it's renamed.
pub const DEFAULT_NAME: &str = "Default";

/// What separates the layers of a path: `Structure/Walls`.
pub const SEPARATOR: char = '/';

/// One layer's state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub id: LayerId,
    pub name: String,
    pub visible: bool,
    /// Its markups can be seen but not picked, moved or changed.
    pub locked: bool,
    /// The layer it is inside, if any.
    #[serde(default)]
    pub parent: Option<LayerId>,
    /// A colour to tell it apart by in a list. It changes nothing drawn.
    #[serde(default)]
    pub colour: Option<[f32; 3]>,
}

impl Layer {
    pub fn new(id: LayerId, name: impl Into<String>) -> Layer {
        Layer { id, name: name.into(), visible: true, locked: false, parent: None, colour: None }
    }
}

/// Where a layer dropped on another one goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// In front of it, beside it.
    Above(LayerId),
    /// Behind it, beside it.
    Below(LayerId),
    /// Inside it, in front of what is there.
    Into(LayerId),
}

/// A document's layers. `layers` is the whole state; `order` and `ranks` are
/// worked out from it whenever it changes.
#[derive(Clone, Debug, PartialEq)]
pub struct LayerStack {
    layers: Vec<Layer>,
    /// Every layer, back to front as drawn: each before the layers inside it.
    order: Vec<LayerId>,
    ranks: HashMap<LayerId, usize>,
}

impl Default for LayerStack {
    fn default() -> LayerStack {
        LayerStack::from_layers([])
    }
}

/// What to make of a name typed for a layer: no separators, no edge spaces.
fn clean(name: &str) -> String {
    name.replace(SEPARATOR, "-").trim().to_owned()
}

impl LayerStack {
    /// Layers in the order given, with the default one added at the bottom if
    /// it's missing, any repeated ID dropped, and any parent that isn't there
    /// -- or that would make a loop -- let go.
    pub fn from_layers(layers: impl IntoIterator<Item = Layer>) -> LayerStack {
        let mut stack = LayerStack { layers: Vec::new(), order: Vec::new(), ranks: HashMap::new() };
        for layer in layers {
            if stack.position(layer.id).is_none() {
                stack.layers.push(layer);
            }
        }
        if stack.position(LayerId::DEFAULT).is_none() {
            stack.layers.insert(0, Layer::new(LayerId::DEFAULT, DEFAULT_NAME));
        }
        stack.reindex();
        stack
    }

    fn reindex(&mut self) {
        let ids: HashSet<LayerId> = self.layers.iter().map(|l| l.id).collect();
        for layer in &mut self.layers {
            // The default layer stays at the top level, and a parent must exist.
            if layer.id.is_default() || layer.parent.is_some_and(|p| p == layer.id || !ids.contains(&p)) {
                layer.parent = None;
            }
        }
        let mut order = Vec::with_capacity(self.layers.len());
        let mut seen = HashSet::new();
        self.walk(None, &mut order, &mut seen);
        // Layers left over are in a loop of parents, which is broken.
        let stuck: Vec<LayerId> = self.layers.iter().map(|l| l.id).filter(|id| !seen.contains(id)).collect();
        for id in stuck {
            if !seen.contains(&id) {
                if let Some(i) = self.position(id) {
                    self.layers[i].parent = None;
                }
                seen.insert(id);
                order.push(id);
                self.walk(Some(id), &mut order, &mut seen);
            }
        }
        self.ranks = order.iter().enumerate().map(|(i, &id)| (id, i)).collect();
        self.order = order;
    }

    fn walk(&self, parent: Option<LayerId>, order: &mut Vec<LayerId>, seen: &mut HashSet<LayerId>) {
        for layer in self.layers.iter().filter(|l| l.parent == parent) {
            if seen.insert(layer.id) {
                order.push(layer.id);
                self.walk(Some(layer.id), order, seen);
            }
        }
    }

    /// Every layer, in the order they were made and moved, not the order they
    /// draw in: see `back_to_front`.
    pub fn layers(&self) -> &[Layer] {
        &self.layers
    }

    /// Every layer back to front as drawn, each before those inside it.
    pub fn back_to_front(&self) -> &[LayerId] {
        &self.order
    }

    /// Every layer with how deep it sits, front to back as a list reads: a
    /// folder, then what is inside it, front first.
    pub fn top_first(&self) -> Vec<(&Layer, usize)> {
        fn walk<'a>(stack: &'a LayerStack, parent: Option<LayerId>, depth: usize, out: &mut Vec<(&'a Layer, usize)>) {
            for layer in stack.layers.iter().rev().filter(|l| l.parent == parent) {
                out.push((layer, depth));
                walk(stack, Some(layer.id), depth + 1, out);
            }
        }
        let mut out = Vec::with_capacity(self.layers.len());
        walk(self, None, 0, &mut out);
        out
    }

    pub fn position(&self, id: LayerId) -> Option<usize> {
        self.layers.iter().position(|l| l.id == id)
    }

    pub fn get(&self, id: LayerId) -> Option<&Layer> {
        self.position(id).map(|i| &self.layers[i])
    }

    pub fn parent_of(&self, id: LayerId) -> Option<LayerId> {
        self.get(id).and_then(|l| l.parent)
    }

    /// The layers directly inside `id`, back to front.
    pub fn children(&self, id: LayerId) -> Vec<LayerId> {
        self.layers.iter().filter(|l| l.parent == Some(id)).map(|l| l.id).collect()
    }

    /// Whether `id` is inside `folder`, at any depth.
    pub fn is_inside(&self, id: LayerId, folder: LayerId) -> bool {
        let mut at = self.parent_of(id);
        while let Some(parent) = at {
            if parent == folder {
                return true;
            }
            at = self.parent_of(parent);
        }
        false
    }

    /// How many layers deep `id` sits: 0 at the top level.
    pub fn depth(&self, id: LayerId) -> usize {
        let mut depth = 0;
        let mut at = self.parent_of(id);
        while let Some(parent) = at {
            depth += 1;
            at = self.parent_of(parent);
        }
        depth
    }

    /// `id` and every layer it is inside, innermost first.
    fn with_folders(&self, id: LayerId) -> Vec<&Layer> {
        let mut out = Vec::new();
        let mut at = self.get(id);
        while let Some(layer) = at {
            out.push(layer);
            at = layer.parent.and_then(|p| self.get(p));
        }
        out
    }

    /// The first layer called `name` directly inside `parent` (the top level
    /// for `None`), ignoring case.
    pub fn by_name(&self, parent: Option<LayerId>, name: &str) -> Option<LayerId> {
        self.layers.iter().find(|l| l.parent == parent && l.name.eq_ignore_ascii_case(name)).map(|l| l.id)
    }

    /// The layer a path names -- `Structure/Walls`, or a lone `Notes` --
    /// wherever it is, if there is one.
    pub fn by_path(&self, path: &str) -> Option<LayerId> {
        let mut at = None;
        for part in path.split(SEPARATOR).map(str::trim).filter(|p| !p.is_empty()) {
            at = Some(self.by_name(at, part)?);
        }
        at
    }

    /// A layer's name as a path from the top: `Structure/Walls`.
    pub fn path(&self, id: LayerId) -> String {
        let mut names: Vec<&str> = self.with_folders(id).iter().map(|l| l.name.as_str()).collect();
        names.reverse();
        names.join("/")
    }

    /// What to show for a layer in a list.
    pub fn name(&self, id: LayerId) -> &str {
        self.get(id).map_or(DEFAULT_NAME, |l| &l.name)
    }

    /// Whether markups on `id` are drawn: it and every folder it is in are
    /// shown. A layer the list has never heard of is shown, so a markup is
    /// never lost to a missing entry.
    pub fn is_visible(&self, id: LayerId) -> bool {
        self.with_folders(id).iter().all(|l| l.visible)
    }

    /// Whether it or a folder it is in is locked.
    pub fn is_locked(&self, id: LayerId) -> bool {
        self.with_folders(id).iter().any(|l| l.locked)
    }

    /// Puts a layer on top of its level, unless its ID is already listed.
    /// Gives whether it was added.
    pub fn insert(&mut self, layer: Layer) -> bool {
        if self.position(layer.id).is_some() {
            return false;
        }
        self.layers.push(layer);
        self.reindex();
        true
    }

    /// A new layer at the front of the top level, called `name` or, if that
    /// is taken there, `name 2`, `name 3`...
    pub fn add(&mut self, name: &str) -> LayerId {
        self.add_in(None, name)
    }

    /// A new layer at the front of `parent`.
    pub fn add_in(&mut self, parent: Option<LayerId>, name: &str) -> LayerId {
        let id = LayerId::new();
        let mut layer = Layer::new(id, self.unique_name(parent, name));
        layer.parent = parent.filter(|p| self.get(*p).is_some());
        self.layers.push(layer);
        self.reindex();
        id
    }

    /// The layer a path names, and every folder on the way, made where the
    /// document hasn't got them. For a preset or a file that names its layer
    /// in text: what is made has the ID its path always gets, so reading the
    /// same file again finds the same layer. An empty path is the default
    /// layer.
    pub fn named_path(&mut self, path: &str) -> LayerId {
        let mut parent = None;
        let mut so_far = String::new();
        for part in path.split(SEPARATOR).map(clean).filter(|p| !p.is_empty()) {
            if !so_far.is_empty() {
                so_far.push(SEPARATOR);
            }
            so_far.push_str(&part);
            parent = Some(match self.by_name(parent, &part) {
                Some(id) => id,
                None => {
                    let id = LayerId::from_name(&so_far.to_lowercase());
                    let mut layer = Layer::new(id, part);
                    layer.parent = parent;
                    if self.position(id).is_none() {
                        self.layers.push(layer);
                        self.reindex();
                        id
                    } else {
                        self.add_in(parent, &so_far)
                    }
                }
            });
        }
        parent.unwrap_or(LayerId::DEFAULT)
    }

    /// `base`, or the first of `base 2`, `base 3`... no layer directly inside
    /// `parent` has.
    pub fn unique_name(&self, parent: Option<LayerId>, base: &str) -> String {
        let base = clean(base);
        let base = if base.is_empty() { "Layer".to_owned() } else { base };
        if self.by_name(parent, &base).is_none() {
            return base;
        }
        (2..).map(|n| format!("{base} {n}")).find(|name| self.by_name(parent, name).is_none()).unwrap()
    }

    /// Takes a layer out, unless it's the default. What was inside it moves
    /// out to where it was, and its markups are the caller's to refile.
    pub fn remove(&mut self, id: LayerId) -> Option<Layer> {
        let i = self.position(id).filter(|_| !id.is_default())?;
        let layer = self.layers.remove(i);
        for inside in self.layers.iter_mut().filter(|l| l.parent == Some(id)) {
            inside.parent = layer.parent;
        }
        self.reindex();
        Some(layer)
    }

    /// Renames a layer, to anything but nothing.
    pub fn rename(&mut self, id: LayerId, to: &str) -> bool {
        let to = clean(to);
        match self.position(id) {
            Some(i) if !to.is_empty() => {
                self.layers[i].name = to;
                true
            }
            _ => false,
        }
    }

    pub fn set_visible(&mut self, id: LayerId, visible: bool) -> bool {
        self.set(id, |l| l.visible = visible)
    }

    pub fn set_locked(&mut self, id: LayerId, locked: bool) -> bool {
        self.set(id, |l| l.locked = locked)
    }

    pub fn set_colour(&mut self, id: LayerId, colour: Option<[f32; 3]>) -> bool {
        self.set(id, |l| l.colour = colour)
    }

    fn set(&mut self, id: LayerId, change: impl FnOnce(&mut Layer)) -> bool {
        match self.position(id) {
            Some(i) => {
                change(&mut self.layers[i]);
                true
            }
            None => false,
        }
    }

    /// Puts layer `id` where a drop says, with what is inside it. Not the
    /// default layer, which stays at the top level, and not into itself or
    /// anything inside it. Gives whether anything moved.
    pub fn place(&mut self, id: LayerId, drop: Place) -> bool {
        let target = match drop {
            Place::Above(t) | Place::Below(t) | Place::Into(t) => t,
        };
        // The default layer stays at the top level.
        let stays_out = id.is_default() && (matches!(drop, Place::Into(_)) || self.parent_of(target).is_some());
        if target == id || stays_out || self.get(target).is_none() || self.is_inside(target, id) {
            return false;
        }
        let before = self.layers.clone();
        let Some(from) = self.position(id) else { return false };
        let mut layer = self.layers.remove(from);
        let Some(at) = self.position(target) else { return false };
        match drop {
            Place::Into(_) => {
                layer.parent = Some(target);
                self.layers.push(layer);
            }
            Place::Above(_) => {
                layer.parent = self.layers[at].parent;
                self.layers.insert(at + 1, layer);
            }
            Place::Below(_) => {
                layer.parent = self.layers[at].parent;
                self.layers.insert(at, layer);
            }
        }
        self.reindex();
        self.layers != before
    }

    /// Moves a layer to `to` in the list of every layer, clamped to it.
    pub fn move_to(&mut self, id: LayerId, to: usize) -> bool {
        let Some(from) = self.position(id) else { return false };
        let layer = self.layers.remove(from);
        self.layers.insert(to.min(self.layers.len()), layer);
        self.reindex();
        true
    }

    /// Where a layer's markups stand among all layers', back 0. Layers not
    /// listed sit above every listed one.
    pub fn rank(&self, id: LayerId) -> usize {
        self.ranks.get(&id).copied().unwrap_or(self.order.len())
    }

    /// The order to draw `markups` in, back first: by layer, then by `z`, then
    /// by ID so ties come out the same every time.
    pub fn sort_back_to_front(&self, markups: &mut [&Markup]) {
        markups.sort_by(|a, b| self.stacking(a, b));
    }

    /// Which of two markups is further back: `Less` if `a` is.
    pub fn stacking(&self, a: &Markup, b: &Markup) -> std::cmp::Ordering {
        self.rank(a.layer).cmp(&self.rank(b.layer)).then(a.extras.z.total_cmp(&b.extras.z)).then(a.id.cmp(&b.id))
    }

    /// Whether `m` is drawn.
    pub fn shows(&self, m: &Markup) -> bool {
        self.is_visible(m.layer)
    }

    /// Whether `m` can be picked and changed: shown and not locked.
    pub fn is_editable(&self, m: &Markup) -> bool {
        self.is_visible(m.layer) && !self.is_locked(m.layer)
    }
}

/// Where a markup goes in its layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Restack {
    ToFront,
    ToBack,
    /// One place forward, past the next markup in front of it.
    Forward,
    /// One place back.
    Backward,
}

/// The `z` to give `id` to restack it among `layer_mates` -- the markups of
/// its layer, whatever the caller stacks within -- or `None` if it's already
/// there or isn't among them. Only the one markup changes: the new `z` lies
/// beyond or between its neighbours'.
pub fn restacked(layer_mates: &[&Markup], id: MarkupId, how: Restack) -> Option<f64> {
    let mut order: Vec<&Markup> = layer_mates.to_vec();
    order.sort_by(|a, b| a.extras.z.total_cmp(&b.extras.z).then(a.id.cmp(&b.id)));
    let at = order.iter().position(|m| m.id == id)?;
    let z = |i: usize| order[i].extras.z;
    let new = match how {
        Restack::ToFront if at + 1 < order.len() => z(order.len() - 1) + 1.0,
        Restack::ToBack if at > 0 => z(0) - 1.0,
        Restack::Forward if at + 1 < order.len() => match order.get(at + 2) {
            Some(after) => (z(at + 1) + after.extras.z) / 2.0,
            None => z(at + 1) + 1.0,
        },
        Restack::Backward if at > 0 => match at.checked_sub(2) {
            Some(before) => (z(at - 1) + z(before)) / 2.0,
            None => z(at - 1) - 1.0,
        },
        _ => return None,
    };
    Some(new)
}

/// The `z` for a markup drawn now: in front of everything in `layer_mates`.
pub fn next_z(layer_mates: &[&Markup]) -> f64 {
    layer_mates.iter().map(|m| m.extras.z).fold(-1.0, f64::max) + 1.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Pt;
    use crate::markup::{Geometry, MarkupKind};

    fn on(layer: LayerId, z: f64) -> Markup {
        let mut m = Markup::new(0, MarkupKind::Length, Geometry::Line { a: Pt::new(0.0, 0.0), b: Pt::new(1.0, 0.0) });
        m.layer = layer;
        m.extras.z = z;
        m
    }

    fn names(stack: &LayerStack) -> Vec<(String, usize)> {
        stack.top_first().into_iter().map(|(l, depth)| (l.name.clone(), depth)).collect()
    }

    #[test]
    fn the_default_layer_is_always_there_and_stays_at_the_top_level() {
        let walls = Layer::new(LayerId::new(), "Walls");
        let mut stack = LayerStack::from_layers([walls.clone()]);
        assert_eq!(stack.layers()[0].id, LayerId::DEFAULT, "added at the bottom");
        assert!(stack.remove(LayerId::DEFAULT).is_none());
        assert!(!stack.insert(walls.clone()), "no second one");
        assert!(stack.rename(LayerId::DEFAULT, "Base"), "it can be renamed, not taken out");
        assert!(!stack.place(LayerId::DEFAULT, Place::Into(walls.id)), "or put inside another");
    }

    #[test]
    fn renaming_keeps_the_id_and_names_stay_unique_where_made() {
        let mut stack = LayerStack::default();
        let walls = stack.add("Walls");
        let second = stack.add("walls");
        assert_eq!(stack.name(second), "walls 2");
        assert!(stack.rename(walls, "Struc/ture"));
        assert_eq!(stack.name(walls), "Struc-ture", "no separator in a name");
        assert!(!stack.rename(walls, "  "));
        // The same name inside another layer is not a clash.
        let inside = stack.add_in(Some(walls), "walls");
        assert_eq!(stack.name(inside), "walls");
    }

    #[test]
    fn a_path_names_a_layer_and_makes_the_folders_on_the_way() {
        let (mut a, mut b) = (LayerStack::default(), LayerStack::default());
        let walls = a.named_path("Structure / Walls");
        assert_eq!(walls, b.named_path("structure/walls"), "the same layer wherever it is made");
        assert_eq!(a.path(walls), "Structure/Walls");
        assert_eq!(a.depth(walls), 1);
        assert_eq!(a.named_path("STRUCTURE/walls"), walls, "found, not made again");
        assert_eq!(a.layers().len(), 3, "the default, the folder and the layer");
        assert_eq!(a.by_path("Structure/Walls"), Some(walls));
        assert_eq!(a.by_path("Structure/Floors"), None);
        assert_eq!(a.named_path(""), LayerId::DEFAULT);
    }

    #[test]
    fn layers_stack_then_z_within_them_and_a_folder_draws_under_what_is_in_it() {
        let mut stack = LayerStack::default();
        let (walls, notes) = (stack.add("Walls"), stack.add("Notes"));
        let inside = stack.add_in(Some(walls), "Doors");
        let (a, b, c, d, e) = (on(notes, 0.0), on(walls, 5.0), on(walls, 1.0), on(LayerId::DEFAULT, 9.0), on(inside, 0.0));
        let expected = [d.id, c.id, b.id, e.id, a.id];
        let mut all = vec![&a, &b, &c, &d, &e];
        stack.sort_back_to_front(&mut all);
        assert_eq!(all.iter().map(|m| m.id).collect::<Vec<_>>(), expected);
        assert_eq!(names(&stack), [("Notes".into(), 0), ("Walls".into(), 0), ("Doors".into(), 1), ("Default".into(), 0)]);
    }

    #[test]
    fn hiding_or_locking_a_folder_reaches_what_is_inside_and_leaves_its_own_settings() {
        let mut stack = LayerStack::default();
        let folder = stack.add("Structure");
        let inside = stack.add_in(Some(folder), "Walls");
        stack.set_visible(inside, false);
        stack.set_visible(folder, false);
        stack.set_visible(folder, true);
        assert!(!stack.is_visible(inside), "what was hidden inside is still hidden");
        stack.set_visible(inside, true);
        stack.set_locked(folder, true);
        let m = on(inside, 0.0);
        assert!(stack.shows(&m) && !stack.is_editable(&m), "locked by the folder");
        stack.set_locked(folder, false);
        stack.set_visible(folder, false);
        assert!(!stack.shows(&m) && !stack.is_editable(&m));
        assert!(stack.shows(&on(LayerId::new(), 0.0)), "an unlisted layer is never lost");
    }

    #[test]
    fn dropping_a_layer_reorders_or_nests_it_with_what_is_inside() {
        let mut stack = LayerStack::default();
        let (a, b, c) = (stack.add("A"), stack.add("B"), stack.add("C"));
        assert_eq!(names(&stack).iter().map(|n| n.0.as_str()).collect::<Vec<_>>(), ["C", "B", "A", "Default"]);
        assert!(stack.place(a, Place::Above(c)));
        assert_eq!(names(&stack).iter().map(|n| n.0.as_str()).collect::<Vec<_>>(), ["A", "C", "B", "Default"]);
        assert!(stack.place(b, Place::Into(a)));
        assert_eq!(names(&stack), [("A".into(), 0), ("B".into(), 1), ("C".into(), 0), ("Default".into(), 0)]);
        // A folder moves with what is inside it, and can't go into its own.
        assert!(!stack.place(a, Place::Into(b)));
        assert!(!stack.place(a, Place::Above(a)));
        assert!(stack.place(a, Place::Below(LayerId::DEFAULT)));
        assert!(stack.rank(a) < stack.rank(LayerId::DEFAULT), "behind the default layer now");
        assert_eq!(stack.parent_of(b), Some(a));
        assert!(!stack.place(c, Place::Above(c)));
    }

    #[test]
    fn taking_a_folder_out_lets_what_was_inside_out_to_where_it_was() {
        let mut stack = LayerStack::default();
        let outer = stack.add("Outer");
        let middle = stack.add_in(Some(outer), "Middle");
        let inner = stack.add_in(Some(middle), "Inner");
        stack.remove(middle);
        assert_eq!(stack.parent_of(inner), Some(outer));
        assert!(stack.get(middle).is_none());
        assert_eq!(stack.by_path("Outer/Inner"), Some(inner));
    }

    #[test]
    fn a_loop_of_parents_read_from_a_file_is_broken() {
        let (x, y) = (LayerId::new(), LayerId::new());
        let mut a = Layer::new(x, "X");
        let mut b = Layer::new(y, "Y");
        (a.parent, b.parent) = (Some(y), Some(x));
        let stack = LayerStack::from_layers([a, b, Layer::new(LayerId::new(), "Z")]);
        assert_eq!(stack.back_to_front().len(), 4, "every layer is still there, once");
    }

    #[test]
    fn restacking_touches_one_markup() {
        let (a, b, c) = (on(LayerId::DEFAULT, 0.0), on(LayerId::DEFAULT, 1.0), on(LayerId::DEFAULT, 2.0));
        let mates = [&a, &b, &c];
        assert_eq!(restacked(&mates, a.id, Restack::ToFront), Some(3.0));
        assert_eq!(restacked(&mates, c.id, Restack::ToFront), None, "already there");
        assert_eq!(restacked(&mates, c.id, Restack::ToBack), Some(-1.0));
        assert_eq!(restacked(&mates, a.id, Restack::Forward), Some(1.5), "between b and c");
        assert_eq!(restacked(&mates, b.id, Restack::Forward), Some(3.0));
        assert_eq!(restacked(&mates, c.id, Restack::Backward), Some(0.5));
        assert_eq!(restacked(&mates, b.id, Restack::Backward), Some(-1.0));
        assert_eq!(next_z(&mates), 3.0);
        assert_eq!(next_z(&[]), 0.0);
    }
}
