//! Layers as the file's own optional content groups.
//!
//! Each layer is written as an optional content group -- the thing other
//! viewers list as a layer and turn on and off -- named for the layer and
//! carrying its stable ID in a /KPDF entry, and each markup on it is tagged
//! with its group in /OC, so hiding the layer in another program hides the
//! markup. On or off is the configuration's /OFF, locked is its /Locked, and
//! the panel order is its /Order, top layer first.
//!
//! Groups the file came with, from a program that draws in layers, have no
//! /KPDF entry and are left exactly as they are: never listed here, and kept
//! in the arrays a save rewrites.
//!
//! A layer inside another names it in /KPDF /Parent. Other viewers get each
//! layer's own on and off, so a folder hidden here shows its layers on there;
//! inside this app the folder's state reaches what is in it.

use std::collections::{HashMap, HashSet};

use markup_model::{Layer, LayerId, LayerStack};
use pdf_content::lopdf::{dictionary, Dictionary, Document, IncrementalDocument, Object, ObjectId};

use crate::values::{get, numbers, read_name, read_text, reals, text};
use crate::Error;

/// What a file holds of layers, before a save.
pub struct Catalog {
    root: ObjectId,
    /// /OCProperties, with its /D read in.
    properties: Dictionary,
    /// Our groups, by layer.
    groups: HashMap<LayerId, ObjectId>,
}

/// Every group in /OCGs, with its object.
fn groups_of<'a>(doc: &'a Document, properties: &'a Dictionary) -> Vec<(ObjectId, &'a Dictionary)> {
    let Some(list) = get(doc, properties, b"OCGs").and_then(|o| o.as_array().ok()) else { return Vec::new() };
    list.iter().filter_map(|o| Some((o.as_reference().ok()?, doc.get_dictionary(o.as_reference().ok()?).ok()?))).collect()
}

/// The layer a group is, if this app wrote it.
fn our_id(doc: &Document, group: &Dictionary) -> Option<LayerId> {
    let kpdf = get(doc, group, b"KPDF")?.as_dict().ok()?;
    read_text(doc, kpdf, b"LayerId").and_then(|nm| LayerId::from_nm(&nm))
}

impl Catalog {
    pub fn of(doc: &Document) -> Option<Catalog> {
        let root = doc.trailer.get(b"Root").ok()?.as_reference().ok()?;
        let catalog = doc.get_dictionary(root).ok()?;
        let mut properties = get(doc, catalog, b"OCProperties").and_then(|o| o.as_dict().ok()).cloned().unwrap_or_default();
        let config = get(doc, &properties, b"D").and_then(|o| o.as_dict().ok()).cloned();
        if let Some(config) = config {
            properties.set("D", config);
        }
        let groups = groups_of(doc, &properties).into_iter().filter_map(|(object, group)| Some((our_id(doc, group)?, object))).collect();
        Some(Catalog { root, properties, groups })
    }

    /// The groups this app wrote, by layer.
    pub fn groups(&self) -> &HashMap<LayerId, ObjectId> {
        &self.groups
    }
}

/// The file's layers, bottom first: those this app wrote, as another program
/// may since have turned them on or off. Just the default layer if it has
/// none.
pub fn read(doc: &Document) -> LayerStack {
    let Some(catalog) = Catalog::of(doc) else { return LayerStack::default() };
    let config = get(doc, &catalog.properties, b"D").and_then(|o| o.as_dict().ok());
    let refs = |key: &[u8]| -> HashSet<ObjectId> {
        config
            .and_then(|c| get(doc, c, key))
            .and_then(|o| o.as_array().ok())
            .map(|a| a.iter().filter_map(|g| g.as_reference().ok()).collect())
            .unwrap_or_default()
    };
    let base_off = config.and_then(|c| read_name(doc, c, b"BaseState")) == Some(b"OFF");
    let (on, off, locked) = (refs(b"ON"), refs(b"OFF"), refs(b"Locked"));
    let layers = groups_of(doc, &catalog.properties).into_iter().filter_map(|(object, group)| {
        Some(Layer {
            id: our_id(doc, group)?,
            name: read_text(doc, group, b"Name").unwrap_or_default(),
            visible: if base_off { on.contains(&object) } else { !off.contains(&object) },
            locked: locked.contains(&object),
            parent: parent_of(doc, group),
            colour: colour_of(doc, group),
        })
    });
    LayerStack::from_layers(layers)
}

/// The colour a group says its layer is marked with, if it does.
fn colour_of(doc: &Document, group: &Dictionary) -> Option<[f32; 3]> {
    let kpdf = get(doc, group, b"KPDF")?.as_dict().ok()?;
    match numbers(doc, kpdf.get(b"Colour").ok()?)?[..] {
        [r, g, b] => Some([r as f32, g as f32, b as f32]),
        _ => None,
    }
}

/// The layer a group says it is inside, if it does.
fn parent_of(doc: &Document, group: &Dictionary) -> Option<LayerId> {
    let kpdf = get(doc, group, b"KPDF")?.as_dict().ok()?;
    read_text(doc, kpdf, b"Parent").and_then(|nm| LayerId::from_nm(&nm))
}

/// `items` without our groups, nested arrays included.
fn without(items: Vec<Object>, ours: &HashSet<ObjectId>) -> Vec<Object> {
    items
        .into_iter()
        .filter_map(|item| match item {
            Object::Reference(id) if ours.contains(&id) => None,
            Object::Array(inner) => Some(Object::Array(without(inner, ours))),
            other => Some(other),
        })
        .collect()
}

fn array_of(dict: &Dictionary, key: &[u8]) -> Vec<Object> {
    dict.get(key).ok().and_then(|o| o.as_array().ok()).cloned().unwrap_or_default()
}

/// Writes `stack` as the file's layers: a group for each, made or changed,
/// and the catalog's lists made to say which are on, locked and where. Gives
/// each layer's group, for tagging the markups on it.
pub fn write(update: &mut IncrementalDocument, catalog: &Catalog, stack: &LayerStack) -> Result<HashMap<LayerId, ObjectId>, Error> {
    let mut groups = catalog.groups.clone();
    for layer in stack.layers() {
        let mut kpdf = dictionary! { "V" => 1, "LayerId" => text(&layer.id.to_nm()) };
        if let Some(parent) = layer.parent {
            kpdf.set("Parent", text(&parent.to_nm()));
        }
        if let Some(colour) = layer.colour {
            kpdf.set("Colour", reals(colour.map(f64::from)));
        }
        match groups.get(&layer.id) {
            Some(&object) => {
                update.opt_clone_object_to_new_document(object)?;
                let group = update.new_document.get_dictionary_mut(object)?;
                group.set("Name", text(&layer.name));
                group.set("KPDF", kpdf);
            }
            None => {
                let object = update.new_document.add_object(dictionary! { "Type" => "OCG", "Name" => text(&layer.name), "KPDF" => kpdf });
                groups.insert(layer.id, object);
            }
        }
    }

    // Layers taken out of the list lose their place in every array too; their
    // groups stay in the file, listed nowhere.
    let ours: HashSet<ObjectId> = catalog.groups.values().chain(groups.values()).copied().collect();
    let reference = |layer: &Layer| Object::Reference(groups[&layer.id]);
    let listed = stack.layers();

    let mut properties = catalog.properties.clone();
    let mut all = without(array_of(&properties, b"OCGs"), &ours);
    let foreign: Vec<Object> = all.clone();
    all.extend(listed.iter().map(reference));
    properties.set("OCGs", all);

    let mut config = properties.get(b"D").ok().and_then(|o| o.as_dict().ok()).cloned().unwrap_or_default();
    let base_off = config.get(b"BaseState").ok().and_then(|o| o.as_name().ok()) == Some(b"OFF");
    let mut off = without(array_of(&config, b"OFF"), &ours);
    off.extend(listed.iter().filter(|l| !l.visible).map(reference));
    config.set("OFF", off);
    if base_off {
        let mut on = without(array_of(&config, b"ON"), &ours);
        on.extend(listed.iter().filter(|l| l.visible).map(reference));
        config.set("ON", on);
    }
    let mut locked = without(array_of(&config, b"Locked"), &ours);
    locked.extend(listed.iter().filter(|l| l.locked).map(reference));
    config.set("Locked", locked);
    // Top layer first, as a viewer lists them. Where the file had no order,
    // its own groups follow ours, or a viewer would list only ours.
    let mut order = if config.has(b"Order") { without(array_of(&config, b"Order"), &ours) } else { foreign };
    order.splice(0..0, nested(stack, None, &groups));
    config.set("Order", order);
    properties.set("D", config);

    update.opt_clone_object_to_new_document(catalog.root)?;
    update.new_document.get_dictionary_mut(catalog.root)?.set("OCProperties", properties);
    Ok(groups)
}

/// The layers inside `parent`, front first, each followed by an array of what is
/// inside it: how /Order shows layers in layers.
fn nested(stack: &LayerStack, parent: Option<LayerId>, groups: &HashMap<LayerId, ObjectId>) -> Vec<Object> {
    let mut out = Vec::new();
    for layer in stack.layers().iter().rev().filter(|l| l.parent == parent) {
        out.push(Object::Reference(groups[&layer.id]));
        let inside = nested(stack, Some(layer.id), groups);
        if !inside.is_empty() {
            out.push(Object::Array(inside));
        }
    }
    out
}

/// Puts an annotation on its layer: /OC to the layer's group, and the layer's
/// ID in /KPDF for the layers no group is written for. One left on another
/// program's group is left there; one of ours the markup has left is taken
/// off.
pub fn tag(annot: &mut Dictionary, layer: LayerId, groups: &HashMap<LayerId, ObjectId>) {
    match groups.get(&layer) {
        Some(&group) => annot.set("OC", Object::Reference(group)),
        None => {
            let ours = annot.get(b"OC").ok().and_then(|o| o.as_reference().ok()).is_some_and(|g| groups.values().any(|&o| o == g));
            if ours {
                annot.remove(b"OC");
            }
        }
    }
    if !layer.is_default() {
        if let Ok(Object::Dictionary(kpdf)) = annot.get_mut(b"KPDF") {
            kpdf.set("LayerId", text(&layer.to_nm()));
        }
    }
}
