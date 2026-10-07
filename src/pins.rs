//! Named navigation points, stored on their PDF pages so reordering, copying
//! and deleting pages carry their pins with them. Markers are a viewer overlay.

use std::collections::HashSet;
use pdf_content::lopdf::{Document, IncrementalDocument, Object, StringFormat};
use serde::{Deserialize, Serialize};

const KEY: &[u8] = b"KPDFPins";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pin {
    pub id: String,
    #[serde(skip)]
    pub page: usize,
    /// PDF user-space coordinates; rotating a page does not change these.
    pub position: [f32; 2],
    pub name: String,
    pub colour: [f32; 3],
    pub layer: markup_model::LayerId,
    pub jump_to_position: bool,
}

impl Pin {
    pub fn new(page: usize, position: [f32; 2], name: String, colour: [f32; 3]) -> Self {
        Self { id: markup_model::MarkupId::new().to_nm(), page, position, name, colour, layer: markup_model::LayerId::DEFAULT, jump_to_position: true }
    }
}

pub fn read(doc: &Document) -> Result<Vec<Pin>, String> {
    let mut pins = Vec::new();
    let mut ids = HashSet::new();
    for (page, id) in doc.get_pages().values().enumerate() {
        let dict = doc.get_dictionary(*id).map_err(|e| e.to_string())?;
        let Ok(value) = dict.get(KEY) else { continue };
        let bytes = value.as_str().map_err(|e| format!("Could not read pins on page {}: {e}", page + 1))?;
        let stored: Vec<Pin> = serde_json::from_slice(bytes).map_err(|e| format!("Could not read pins on page {}: {e}", page + 1))?;
        for mut pin in stored {
            if !pin.position.iter().all(|n| n.is_finite()) || !pin.colour.iter().all(|n| n.is_finite() && (0.0..=1.0).contains(n)) {
                return Err(format!("Invalid pin on page {}", page + 1));
            }
            pin.page = page;
            // A copied page carries the original's pins, with fresh identities.
            if pin.id.is_empty() || !ids.insert(pin.id.clone()) {
                pin.id = markup_model::MarkupId::new().to_nm();
                ids.insert(pin.id.clone());
            }
            pins.push(pin);
        }
    }
    Ok(pins)
}

pub fn append(bytes: Vec<u8>, pins: &[Pin]) -> Result<Vec<u8>, String> {
    let previous = Document::load_mem(&bytes).map_err(|e| e.to_string())?;
    let pages = previous.get_pages();
    if pins.iter().any(|pin| pin.page >= pages.len()) {
        return Err("A pin points to a page that is no longer in the document".to_owned());
    }
    let mut update = IncrementalDocument::create_from(bytes, previous);
    for (page, id) in pages.values().enumerate() {
        let on_page: Vec<&Pin> = pins.iter().filter(|pin| pin.page == page).collect();
        if on_page.is_empty() && !update.get_prev_documents().get_dictionary(*id).map_err(|e| e.to_string())?.has(KEY) {
            continue;
        }
        update.opt_clone_object_to_new_document(*id).map_err(|e| e.to_string())?;
        let dict = update.new_document.get_dictionary_mut(*id).map_err(|e| e.to_string())?;
        if on_page.is_empty() {
            dict.remove(KEY);
        } else {
            let data = serde_json::to_vec(&on_page).map_err(|e| e.to_string())?;
            dict.set(KEY, Object::String(data, StringFormat::Hexadecimal));
        }
    }
    let mut out = Vec::new();
    update.save_to(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}
