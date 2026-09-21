//! The order the sheets are in, and the editing of it: taking sheets out,
//! copying and pasting them, duplicating them, inserting blanks, and dragging
//! them into a new order.
//!
//! Nothing here knows about egui, pdfium or the page cache. An `Arrangement`
//! is a list saying which page of the file each sheet shows, plus what the
//! user has picked out and what they last cut or copied; every edit is one
//! method on it, and every method can be undone. The sheet view (see
//! `app/arrange.rs`) draws that list and hands back what the user did.
//!
//! A sheet naming a page costs nothing until the arrangement is written: a
//! duplicate is another sheet naming the same page, so duplicating a 40 MB
//! drawing is a `usize`. `rearrange` is where that becomes a file -- it
//! copies a page's dictionary only where the same page is used twice.
//!
//! What this deliberately does not do yet: the highlights and markups on a
//! page are held against its place in the file (`AnnotKey`), so taking pages
//! out or moving them would move every annotation after them. Until that is
//! sorted out, an arrangement is applied by writing the file and opening it
//! again, which is why `rearrange` takes a whole document.

use std::collections::BTreeSet;

use pdf_content::lopdf::{dictionary, Dictionary, Document, Object, ObjectId};

/// Undo steps kept for the page order. An order is a few bytes a sheet, so
/// even a drawing set's worth of steps is small.
pub const HISTORY: usize = 200;

/// One sheet of the document as it will be written.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sheet {
    /// A page of the file as opened, counted from 0. Two sheets can name the
    /// same page; that is what a duplicate is.
    Page(usize),
    /// A sheet with nothing on it, as wide and tall as this in points --
    /// taken from the sheet it was put next to, so a blank in a drawing set
    /// is a drawing sheet rather than a letter page.
    Blank([f32; 2]),
}

impl Sheet {
    /// The page of the file this shows, if it shows one.
    pub fn page(self) -> Option<usize> {
        match self {
            Sheet::Page(page) => Some(page),
            Sheet::Blank(_) => None,
        }
    }
}

/// What has changed about the order, for telling the user before they apply it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Changed {
    pub removed: usize,
    pub added: usize,
    pub moved: usize,
}

impl Changed {
    pub fn any(self) -> bool {
        self.removed + self.added + self.moved > 0
    }

    /// "2 sheets taken out, 1 moved", or nothing at all.
    pub fn describe(self) -> String {
        let sheets = |n: usize| if n == 1 { "1 sheet".to_owned() } else { format!("{n} sheets") };
        let mut parts = Vec::new();
        if self.removed > 0 {
            parts.push(format!("{} taken out", sheets(self.removed)));
        }
        if self.added > 0 {
            parts.push(format!("{} added", sheets(self.added)));
        }
        if self.moved > 0 {
            parts.push(format!("{} moved", sheets(self.moved)));
        }
        parts.join(", ")
    }
}

/// The sheets, what is picked out, and what can be undone.
#[derive(Clone, Debug, Default)]
pub struct Arrangement {
    sheets: Vec<Sheet>,
    /// The order the file itself is in, so we can tell whether anything has
    /// changed and put it back if the user discards the lot.
    opened_with: Vec<Sheet>,
    /// The sheets picked out, by where they are in `sheets`.
    selected: BTreeSet<usize>,
    /// Where a shift-click measures its range from: the last sheet clicked
    /// without shift.
    anchor: Option<usize>,
    /// What was last cut or copied. Kept through undo, as a clipboard is.
    clipboard: Vec<Sheet>,
    undo: Vec<Step>,
    redo: Vec<Step>,
}

/// An order and what was picked out at the time, so undo puts back both.
#[derive(Clone, Debug)]
struct Step {
    sheets: Vec<Sheet>,
    selected: BTreeSet<usize>,
}

impl Arrangement {
    /// A document of `pages` pages, in the order the file holds them.
    pub fn new(pages: usize) -> Self {
        let sheets: Vec<Sheet> = (0..pages).map(Sheet::Page).collect();
        Arrangement { opened_with: sheets.clone(), sheets, ..Arrangement::default() }
    }

    pub fn sheets(&self) -> &[Sheet] {
        &self.sheets
    }

    pub fn len(&self) -> usize {
        self.sheets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sheets.is_empty()
    }

    /// The page of the file sheet `at` shows, if it shows one.
    pub fn page_of(&self, at: usize) -> Option<usize> {
        self.sheets.get(at).copied().and_then(Sheet::page)
    }

    pub fn selected(&self) -> &BTreeSet<usize> {
        &self.selected
    }

    pub fn is_selected(&self, at: usize) -> bool {
        self.selected.contains(&at)
    }

    pub fn clipboard_is_empty(&self) -> bool {
        self.clipboard.is_empty()
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// What has changed since the file was opened: how many of its pages are
    /// no longer shown, how many sheets are new (duplicates and blanks), and
    /// how few sheets would have to be picked up to put the rest back in the
    /// order the file holds them. That last is what a person means by "one
    /// sheet moved": dragging the first sheet to the end moves one sheet, not
    /// every sheet it passed.
    pub fn changed(&self) -> Changed {
        let was: Vec<usize> = self.opened_with.iter().filter_map(|s| s.page()).collect();
        let mut seen = BTreeSet::new();
        let mut added = 0;
        let mut kept = Vec::new();
        for sheet in &self.sheets {
            match sheet.page() {
                Some(page) if seen.insert(page) => kept.push(page),
                _ => added += 1,
            }
        }
        let removed = was.iter().filter(|page| !seen.contains(page)).count();
        // The pages still shown are in the file's order wherever they run
        // upwards, so the ones that had to be picked up are the rest.
        let moved = kept.len() - longest_run_in_order(&kept);
        Changed { removed, added, moved }
    }

    /// Whether the order differs from the file's at all.
    pub fn edited(&self) -> bool {
        self.sheets != self.opened_with
    }

    /* ------------------------------------------------------------------ *
     * Picking sheets out
     * ------------------------------------------------------------------ */

    /// A click on sheet `at`: on its own it picks that one, with Ctrl it adds
    /// or removes it, and with Shift it takes everything from the last plain
    /// click to here.
    pub fn click(&mut self, at: usize, ctrl: bool, shift: bool) {
        if at >= self.sheets.len() {
            return;
        }
        match (ctrl, shift) {
            (_, true) => {
                let from = self.anchor.unwrap_or(at);
                let range = from.min(at)..=from.max(at);
                if !ctrl {
                    self.selected.clear();
                }
                self.selected.extend(range);
            }
            (true, false) => {
                if !self.selected.remove(&at) {
                    self.selected.insert(at);
                }
                self.anchor = Some(at);
            }
            (false, false) => {
                self.selected.clear();
                self.selected.insert(at);
                self.anchor = Some(at);
            }
        }
    }

    pub fn select_all(&mut self) {
        self.selected = (0..self.sheets.len()).collect();
    }

    pub fn clear_selection(&mut self) {
        self.selected.clear();
        self.anchor = None;
    }

    /* ------------------------------------------------------------------ *
     * Editing
     * ------------------------------------------------------------------ */

    /// Remembers the order before a change, for undo.
    fn remember(&mut self) {
        self.undo.push(Step { sheets: self.sheets.clone(), selected: self.selected.clone() });
        if self.undo.len() > HISTORY {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// The sheets picked out, in the order they sit in.
    fn taken(&self) -> Vec<Sheet> {
        self.selected.iter().filter_map(|&at| self.sheets.get(at).copied()).collect()
    }

    /// Drops the picked sheets out of the list, leaving the rest in order.
    fn lift(&mut self) {
        let gone = std::mem::take(&mut self.selected);
        let mut at = 0;
        self.sheets.retain(|_| {
            let keep = !gone.contains(&at);
            at += 1;
            keep
        });
    }

    /// Takes the picked sheets out. The last sheet in a document can't be
    /// taken out: a PDF with no pages is not one.
    pub fn delete(&mut self) -> bool {
        if self.selected.is_empty() || self.selected.len() == self.sheets.len() {
            return false;
        }
        self.remember();
        self.lift();
        self.anchor = None;
        true
    }

    /// Copies the picked sheets, ready to paste.
    pub fn copy(&mut self) -> bool {
        if self.selected.is_empty() {
            return false;
        }
        self.clipboard = self.taken();
        true
    }

    /// Copies the picked sheets and takes them out.
    pub fn cut(&mut self) -> bool {
        self.copy() && self.delete()
    }

    /// Pastes what was cut or copied so it starts at `at` (0 to put it before
    /// the first sheet, `len` to put it after the last), and picks out what
    /// was pasted -- which is what the user goes on to drag or delete.
    pub fn paste(&mut self, at: usize) -> bool {
        if self.clipboard.is_empty() {
            return false;
        }
        let at = at.min(self.sheets.len());
        self.remember();
        let pasted = self.clipboard.clone();
        let count = pasted.len();
        self.sheets.splice(at..at, pasted);
        self.selected = (at..at + count).collect();
        self.anchor = Some(at);
        true
    }

    /// Pastes after the last picked sheet, or at the end when nothing is
    /// picked: what Ctrl+V does when there is no drop point to aim at.
    pub fn paste_after_selection(&mut self) -> bool {
        let at = self.selected.iter().next_back().map_or(self.sheets.len(), |&last| last + 1);
        self.paste(at)
    }

    /// Puts a copy of each picked sheet straight after the picked ones, and
    /// picks out the copies.
    pub fn duplicate(&mut self) -> bool {
        if self.selected.is_empty() {
            return false;
        }
        self.remember();
        let copies = self.taken();
        let at = self.selected.iter().next_back().map_or(0, |&last| last + 1);
        let count = copies.len();
        self.sheets.splice(at..at, copies);
        self.selected = (at..at + count).collect();
        self.anchor = Some(at);
        true
    }

    /// Puts an empty sheet at `at` and picks it out.
    pub fn insert_blank(&mut self, at: usize, size: [f32; 2]) -> bool {
        let at = at.min(self.sheets.len());
        self.remember();
        self.sheets.insert(at, Sheet::Blank(size));
        self.selected = [at].into_iter().collect();
        self.anchor = Some(at);
        true
    }

    /// Moves the picked sheets so they start in the gap at `to` -- counted
    /// before anything is taken out, which is the gap the user is looking at
    /// while dragging.
    pub fn move_selected(&mut self, to: usize) -> bool {
        if self.selected.is_empty() {
            return false;
        }
        let to = to.min(self.sheets.len());
        let before = self.selected.iter().filter(|&&at| at < to).count();
        let landing = to - before;
        let moving = self.taken();
        // Dropped back where they already are, having crossed no gap.
        if self.selected.iter().copied().eq(landing..landing + moving.len()) {
            return false;
        }
        self.remember();
        self.lift();
        let count = moving.len();
        self.sheets.splice(landing..landing, moving);
        self.selected = (landing..landing + count).collect();
        self.anchor = Some(landing);
        true
    }

    pub fn undo(&mut self) -> bool {
        let Some(step) = self.undo.pop() else { return false };
        self.redo.push(Step { sheets: self.sheets.clone(), selected: self.selected.clone() });
        self.sheets = step.sheets;
        self.selected = step.selected;
        self.anchor = None;
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(step) = self.redo.pop() else { return false };
        self.undo.push(Step { sheets: self.sheets.clone(), selected: self.selected.clone() });
        self.sheets = step.sheets;
        self.selected = step.selected;
        self.anchor = None;
        true
    }

    /// Back to the order the file holds, in one step that can itself be undone.
    pub fn discard(&mut self) -> bool {
        if !self.edited() {
            return false;
        }
        self.remember();
        self.sheets = self.opened_with.clone();
        self.selected.clear();
        self.anchor = None;
        true
    }
}

/// How long the longest run of `pages` already in increasing order is, the
/// pages of it needing not be next to each other. Patience sorting, so a
/// drawing set's worth of sheets costs microseconds rather than a pass per
/// pair of them.
fn longest_run_in_order(pages: &[usize]) -> usize {
    let mut ends: Vec<usize> = Vec::new();
    for &page in pages {
        match ends.binary_search(&page) {
            Ok(_) => {}
            Err(at) if at == ends.len() => ends.push(page),
            Err(at) => ends[at] = page,
        }
    }
    ends.len()
}

/* ---------------------------------------------------------------------- *
 * Writing an arrangement out
 * ---------------------------------------------------------------------- */

/// The entries a page inherits from the /Pages nodes above it, which stop
/// being inherited once the tree is flattened, so they are written onto the
/// page itself first.
const INHERITED: [&[u8]; 4] = [b"Resources", b"MediaBox", b"CropBox", b"Rotate"];

/// Writes `sheets` as the page order of `doc`, in place: the same file with
/// its pages in a different order, some missing, some twice over, some blank.
///
/// Everything else in the file is left as it is -- fonts, layers, the
/// catalog, annotations on the pages that stay -- and objects nothing reaches
/// any more are pruned. Bookmarks pointing at pages that have gone are left
/// pointing nowhere, as they are in every other program that does this.
pub fn rearrange(doc: &mut Document, sheets: &[Sheet]) -> Result<(), String> {
    if sheets.is_empty() {
        return Err("a PDF has to have at least one page".into());
    }
    let root = match doc.catalog().map_err(|e| e.to_string())?.get(b"Pages") {
        Ok(Object::Reference(id)) => *id,
        _ => return Err("this file's page tree isn't where the catalog says".into()),
    };
    let pages: Vec<ObjectId> = doc.get_pages().values().copied().collect();

    // What a page inherits has to come down onto the page before the tree is
    // flattened under one node, or a page taking its size from a /Pages node
    // above it would lose it.
    for &id in &pages {
        let inherited = inherited_entries(doc, id);
        if let Ok(page) = doc.get_dictionary_mut(id) {
            for (key, value) in inherited {
                page.set(key, value);
            }
        }
    }

    let mut kids = Vec::with_capacity(sheets.len());
    let mut used: BTreeSet<ObjectId> = BTreeSet::new();
    for (place, sheet) in sheets.iter().enumerate() {
        let id = match *sheet {
            Sheet::Page(page) => {
                let id = *pages
                    .get(page)
                    .ok_or_else(|| format!("sheet {} names page {}, which isn't in the file", place + 1, page + 1))?;
                // The same page twice over needs a dictionary of its own the
                // second time: one page object can only sit in the tree once.
                if used.insert(id) {
                    id
                } else {
                    copy_page(doc, id)?
                }
            }
            Sheet::Blank(size) => blank_page(doc, size),
        };
        kids.push(Object::Reference(id));
    }

    for kid in &kids {
        if let Object::Reference(id) = kid {
            if let Ok(page) = doc.get_dictionary_mut(*id) {
                page.set("Parent", Object::Reference(root));
            }
        }
    }
    let count = kids.len() as i64;
    let tree = doc.get_dictionary_mut(root).map_err(|e| e.to_string())?;
    tree.set("Kids", kids);
    tree.set("Count", count);
    // What the pages now carry themselves must not be inherited from here as
    // well, or a blank sheet would take the first tree's size.
    for key in INHERITED {
        tree.remove(key);
    }
    doc.prune_objects();
    Ok(())
}

/// The inheritable entries a page does not have itself, from the nearest
/// /Pages node above it that does.
fn inherited_entries(doc: &Document, page: ObjectId) -> Vec<(Vec<u8>, Object)> {
    let Ok(dict) = doc.get_dictionary(page) else { return Vec::new() };
    let mut found: Vec<(Vec<u8>, Object)> = Vec::new();
    let mut wanted: Vec<&[u8]> = INHERITED.iter().copied().filter(|key| !dict.has(key)).collect();
    let mut at = dict.get(b"Parent").ok().and_then(|p| p.as_reference().ok());
    // A malformed file can point a page at itself; the depth stops that.
    for _ in 0..32 {
        let Some(id) = at else { break };
        if wanted.is_empty() {
            break;
        }
        let Ok(node) = doc.get_dictionary(id) else { break };
        wanted.retain(|key| match node.get(key) {
            Ok(value) => {
                found.push((key.to_vec(), value.clone()));
                false
            }
            Err(_) => true,
        });
        at = node.get(b"Parent").ok().and_then(|p| p.as_reference().ok());
    }
    found
}

/// A second sheet showing the same page: its dictionary again, with
/// annotations of its own so editing one sheet's does not edit the other's.
fn copy_page(doc: &mut Document, page: ObjectId) -> Result<ObjectId, String> {
    let mut dict = doc.get_dictionary(page).map_err(|e| e.to_string())?.clone();
    let annots: Vec<Dictionary> = match dict.get(b"Annots") {
        Ok(Object::Array(items)) => items
            .iter()
            .filter_map(|item| match item {
                Object::Reference(id) => doc.get_dictionary(*id).ok().cloned(),
                Object::Dictionary(d) => Some(d.clone()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    dict.remove(b"Annots");
    let copy = doc.add_object(Object::Dictionary(dict));
    if !annots.is_empty() {
        let copied: Vec<Object> = annots
            .into_iter()
            .map(|mut annot| {
                annot.set("P", Object::Reference(copy));
                Object::Reference(doc.add_object(Object::Dictionary(annot)))
            })
            .collect();
        if let Ok(dict) = doc.get_dictionary_mut(copy) {
            dict.set("Annots", copied);
        }
    }
    Ok(copy)
}

/// A sheet with nothing on it, `size` points across and down.
fn blank_page(doc: &mut Document, size: [f32; 2]) -> ObjectId {
    let media = vec![Object::Real(0.0), Object::Real(0.0), Object::Real(size[0]), Object::Real(size[1])];
    doc.add_object(dictionary! {
        "Type" => "Page",
        "MediaBox" => media,
        "Resources" => Dictionary::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdf_content::lopdf::Stream;

    fn three() -> Arrangement {
        Arrangement::new(3)
    }

    fn order(a: &Arrangement) -> Vec<Option<usize>> {
        a.sheets().iter().map(|s| s.page()).collect()
    }

    fn picked(ats: &[usize]) -> BTreeSet<usize> {
        ats.iter().copied().collect()
    }

    #[test]
    fn a_new_arrangement_is_the_file_in_its_own_order() {
        let a = three();
        assert_eq!(order(&a), vec![Some(0), Some(1), Some(2)]);
        assert!(!a.edited());
        assert!(!a.changed().any());
    }

    #[test]
    fn a_plain_click_picks_one_sheet_and_shift_takes_the_range() {
        let mut a = Arrangement::new(5);
        a.click(1, false, false);
        assert_eq!(a.selected(), &picked(&[1]));
        a.click(3, false, true);
        assert_eq!(a.selected(), &picked(&[1, 2, 3]));
        a.click(0, true, false);
        assert_eq!(a.selected(), &picked(&[0, 1, 2, 3]));
        // Ctrl again on the same sheet takes it back out.
        a.click(0, true, false);
        assert_eq!(a.selected(), &picked(&[1, 2, 3]));
    }

    #[test]
    fn deleting_takes_the_picked_sheets_out() {
        let mut a = three();
        a.click(1, false, false);
        assert!(a.delete());
        assert_eq!(order(&a), vec![Some(0), Some(2)]);
        assert_eq!(a.changed(), Changed { removed: 1, added: 0, moved: 0 });
    }

    #[test]
    fn the_last_sheet_cannot_be_taken_out() {
        let mut a = Arrangement::new(1);
        a.select_all();
        assert!(!a.delete());
        assert_eq!(a.len(), 1);
    }

    #[test]
    fn cut_and_paste_moves_sheets_and_picks_out_what_landed() {
        let mut a = three();
        a.click(0, false, false);
        assert!(a.cut());
        assert_eq!(order(&a), vec![Some(1), Some(2)]);
        assert!(a.paste(2));
        assert_eq!(order(&a), vec![Some(1), Some(2), Some(0)]);
        assert_eq!(a.selected(), &picked(&[2]));
        assert_eq!(a.changed(), Changed { removed: 0, added: 0, moved: 1 });
    }

    #[test]
    fn moving_one_sheet_counts_as_one_sheet_moved_however_far_it_goes() {
        let mut a = Arrangement::new(6);
        a.click(0, false, false);
        a.move_selected(6);
        assert_eq!(a.changed(), Changed { removed: 0, added: 0, moved: 1 });
        // And two sheets swapped: putting either one back is enough.
        let mut a = Arrangement::new(4);
        a.click(0, false, false);
        a.move_selected(2);
        assert_eq!(a.changed().moved, 1);
    }

    #[test]
    fn copying_leaves_the_sheets_where_they_are_and_pasting_duplicates_them() {
        let mut a = three();
        a.click(0, false, false);
        assert!(a.copy());
        assert!(a.paste(3));
        assert_eq!(order(&a), vec![Some(0), Some(1), Some(2), Some(0)]);
        assert_eq!(a.changed(), Changed { removed: 0, added: 1, moved: 0 });
    }

    #[test]
    fn duplicating_puts_the_copies_after_the_picked_sheets() {
        let mut a = three();
        a.click(0, false, false);
        a.click(1, false, true);
        assert!(a.duplicate());
        assert_eq!(order(&a), vec![Some(0), Some(1), Some(0), Some(1), Some(2)]);
        assert_eq!(a.selected(), &picked(&[2, 3]));
    }

    #[test]
    fn a_blank_sheet_goes_in_where_it_is_asked_for() {
        let mut a = three();
        assert!(a.insert_blank(1, [595.0, 842.0]));
        assert_eq!(order(&a), vec![Some(0), None, Some(1), Some(2)]);
        assert_eq!(a.sheets()[1], Sheet::Blank([595.0, 842.0]));
        assert_eq!(a.changed(), Changed { removed: 0, added: 1, moved: 0 });
    }

    #[test]
    fn dragging_sheets_lands_them_in_the_gap_the_user_is_looking_at() {
        // The last sheet dragged to the very front.
        let mut a = Arrangement::new(4);
        a.click(3, false, false);
        assert!(a.move_selected(0));
        assert_eq!(order(&a), vec![Some(3), Some(0), Some(1), Some(2)]);
        // The first two dragged into the gap after the last sheet, counted
        // before they are taken out: they land at the end.
        let mut a = Arrangement::new(4);
        a.click(0, false, false);
        a.click(1, false, true);
        assert!(a.move_selected(4));
        assert_eq!(order(&a), vec![Some(2), Some(3), Some(0), Some(1)]);
        assert_eq!(a.selected(), &picked(&[2, 3]));
    }

    #[test]
    fn dropping_sheets_back_where_they_are_changes_nothing() {
        let mut a = Arrangement::new(4);
        a.click(1, false, false);
        a.click(2, false, true);
        assert!(!a.move_selected(1));
        assert!(!a.move_selected(3));
        assert!(!a.edited());
        assert!(!a.can_undo());
    }

    #[test]
    fn every_edit_can_be_undone_and_done_again() {
        let mut a = three();
        a.click(1, false, false);
        a.delete();
        a.insert_blank(0, [10.0, 10.0]);
        assert_eq!(order(&a), vec![None, Some(0), Some(2)]);
        assert!(a.undo());
        assert_eq!(order(&a), vec![Some(0), Some(2)]);
        assert!(a.undo());
        assert_eq!(order(&a), vec![Some(0), Some(1), Some(2)]);
        assert!(!a.undo());
        assert!(a.redo());
        assert_eq!(order(&a), vec![Some(0), Some(2)]);
    }

    #[test]
    fn discarding_puts_the_file_s_own_order_back() {
        let mut a = three();
        a.click(0, false, false);
        a.delete();
        a.paste_after_selection();
        assert!(a.discard());
        assert_eq!(order(&a), vec![Some(0), Some(1), Some(2)]);
        assert!(!a.edited());
    }

    /* ---------------------------------------------------------------- *
     * Writing
     * ---------------------------------------------------------------- */

    /// A document whose pages each draw one letter, so the order they end up
    /// in can be read back. The pages have no /MediaBox of their own, so this
    /// is a test of inheritance as well as of order.
    fn lettered(letters: &str) -> Document {
        let mut doc = Document::with_version("1.7");
        let tree = doc.new_object_id();
        let kids: Vec<Object> = letters
            .chars()
            .map(|letter| {
                let content = doc.add_object(Stream::new(Dictionary::new(), format!("({letter}) Tj").into_bytes()));
                Object::Reference(doc.add_object(dictionary! {
                    "Type" => "Page",
                    "Parent" => Object::Reference(tree),
                    "Contents" => Object::Reference(content),
                }))
            })
            .collect();
        let count = kids.len() as i64;
        doc.objects.insert(
            tree,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => kids,
                "Count" => count,
                "MediaBox" => vec![Object::Real(0.0), Object::Real(0.0), Object::Real(200.0), Object::Real(100.0)],
            }),
        );
        let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => Object::Reference(tree) });
        doc.trailer.set("Root", Object::Reference(catalog));
        doc
    }

    fn letters_of(doc: &Document) -> String {
        doc.get_pages()
            .values()
            .map(|&id| {
                let content = String::from_utf8_lossy(&doc.get_page_content(id)).into_owned();
                content.trim_start_matches('(').chars().next().unwrap_or('?')
            })
            .collect()
    }

    fn media_box(doc: &Document, id: ObjectId) -> Vec<f32> {
        let media = doc.get_dictionary(id).expect("a page").get(b"MediaBox").expect("a size");
        media.as_array().expect("four numbers").iter().filter_map(|n| n.as_float().ok()).collect()
    }

    #[test]
    fn the_written_file_holds_the_sheets_in_the_order_asked_for() {
        let mut doc = lettered("ABCD");
        rearrange(&mut doc, &[Sheet::Page(3), Sheet::Page(0), Sheet::Page(2)]).expect("it writes");
        assert_eq!(letters_of(&doc), "DAC");
        assert_eq!(doc.get_pages().len(), 3);
    }

    #[test]
    fn a_page_used_twice_is_written_twice() {
        let mut doc = lettered("AB");
        rearrange(&mut doc, &[Sheet::Page(0), Sheet::Page(1), Sheet::Page(0)]).expect("it writes");
        assert_eq!(letters_of(&doc), "ABA");
        let ids: Vec<_> = doc.get_pages().values().copied().collect();
        assert_ne!(ids[0], ids[2], "the same page object cannot sit in the tree twice");
    }

    #[test]
    fn a_page_keeps_the_size_it_inherited_once_the_tree_is_flattened() {
        let mut doc = lettered("ABC");
        rearrange(&mut doc, &[Sheet::Page(2), Sheet::Page(0)]).expect("it writes");
        for &id in doc.get_pages().values() {
            assert_eq!(media_box(&doc, id), vec![0.0, 0.0, 200.0, 100.0]);
        }
    }

    #[test]
    fn a_blank_sheet_is_written_as_an_empty_page_of_the_size_asked_for() {
        let mut doc = lettered("A");
        rearrange(&mut doc, &[Sheet::Page(0), Sheet::Blank([595.0, 842.0])]).expect("it writes");
        let ids: Vec<_> = doc.get_pages().values().copied().collect();
        assert_eq!(ids.len(), 2);
        assert!(doc.get_page_content(ids[1]).is_empty(), "a blank sheet draws nothing");
        assert_eq!(media_box(&doc, ids[1]), vec![0.0, 0.0, 595.0, 842.0]);
    }

    #[test]
    fn what_is_written_can_be_read_back_as_a_pdf() {
        let mut doc = lettered("ABCD");
        rearrange(&mut doc, &[Sheet::Page(1), Sheet::Page(1), Sheet::Blank([10.0, 10.0])]).expect("it writes");
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).expect("it saves");
        let read = Document::load_mem(&bytes).expect("it loads again");
        assert_eq!(read.get_pages().len(), 3);
        assert_eq!(letters_of(&read).get(..2), Some("BB"));
    }

    #[test]
    fn writing_no_sheets_at_all_is_refused() {
        let mut doc = lettered("A");
        assert!(rearrange(&mut doc, &[]).is_err());
    }
}
