//! The open document's highlights and markups, and every change to them.
//!
//! The UI never edits them directly: it hands the session a `Command`, which
//! the session applies and remembers, so any change can be undone and redone.
//! What a save must write -- new annotations, deleted ones, changed notes -- is
//! worked out from the session's state rather than kept in lists of its own,
//! so undoing back to the file as saved leaves nothing to save.
//!
//! Each highlight and markup has a `uid` for as long as the document is open.
//! A save moves annotations within the file, so their keys change; the session
//! matches the pages read back after a save to the same uids, so selection,
//! undo and edits made while the save was running all carry on.
//!
//! Nothing here runs per frame: each command costs a pass over the document's
//! highlights and markups at most.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

use crate::model::{AnnotKey, Changes, Highlight, Markup, NewHighlight, Rgb, ScaleChanges, ScaleStore};

/// Undo steps kept. Older ones are forgotten.
pub const HISTORY: usize = 1000;

/// A highlight as displayed.
#[derive(Clone, Debug)]
pub struct HighlightEntry {
    pub uid: u64,
    pub hl: Highlight,
}

impl HighlightEntry {
    /// Not in the file yet.
    pub fn is_new(&self) -> bool {
        self.hl.key.is_none()
    }
}

/// A markup as displayed.
#[derive(Clone, Debug)]
pub struct MarkupEntry {
    pub uid: u64,
    pub markup: Markup,
}

/// A change the user makes.
#[derive(Clone, Debug)]
pub enum Command {
    /// New highlights, one per page a selection covers, undone together.
    AddHighlights(Vec<Highlight>),
    AddMarkup(Markup),
    /// A highlight's or markup's note, and its colour while it isn't in the
    /// file: a saved one may carry an appearance written elsewhere, which
    /// would keep showing the old colour.
    EditNote { uid: u64, comment: String, color: Rgb },
    Remove(u64),
    /// The document's scales and viewports, as a whole: calibrating a page,
    /// giving pages another page's scale, naming a region. The caller changes
    /// a copy of `scales()` and hands it back, so every way of changing them
    /// undoes the same way. They are small: a few scales and viewports.
    SetScales(ScaleStore),
}

#[derive(Clone, Debug)]
enum Item {
    Highlight(Highlight),
    Markup(Markup),
}

#[derive(Clone, Debug)]
enum Step {
    Added(Vec<u64>),
    Removed(u64),
    Edited { uid: u64, before: (String, Rgb), after: (String, Rgb) },
    Scaled { before: Box<ScaleStore>, after: Box<ScaleStore> },
}

/// Where an annotation is in the file, and its note there.
#[derive(Clone, Debug)]
struct InFile {
    key: AnnotKey,
    comment: String,
}

/// A page and whether markups (true) or highlights (false): how a save
/// groups what it writes and reads back.
type Group = (usize, bool);

/// What a save in progress is writing, to match the pages read back to uids.
#[derive(Debug)]
struct Saving {
    /// Uids whose annotations the save deletes.
    deleted: Vec<u64>,
    /// For each page the save changes, and highlights (false) or markups
    /// (true), the uids of that page's annotations in the order the file will
    /// hold them: those already there, by position, then those added.
    expected: HashMap<Group, Vec<u64>>,
    /// The scales written, which the file then holds.
    scales: Option<Box<ScaleStore>>,
}

#[derive(Debug, Default)]
pub struct Session {
    /// In page order and, within a page, file order, unsaved ones last.
    highlights: Vec<HighlightEntry>,
    markups: Vec<MarkupEntry>,
    /// Highlights and markups taken out, kept for undo and redo, and for a
    /// save to delete those in the file.
    removed: HashMap<u64, Item>,
    /// Every uid that is in the file, removed or not.
    file: HashMap<u64, InFile>,
    undo: VecDeque<Step>,
    redo: Vec<Step>,
    saving: Option<Saving>,
    next_uid: u64,
    /// Whether a save would write anything, as of the last change.
    dirty: bool,
    /// The document's scales and viewports, and how they stood in the file,
    /// so a change to them is part of what's unsaved and can be undone with
    /// everything else.
    scales: ScaleStore,
    file_scales: ScaleStore,
    /// Saved markups removed: the page's drawing shows them until a save.
    erased: Vec<Markup>,
}

/// Where an annotation sorts: by page, then file position, new ones last.
fn order(page: usize, key: Option<AnnotKey>) -> (usize, usize) {
    (page, key.map_or(usize::MAX, |k| k.index))
}

impl Item {
    fn page(&self) -> usize {
        match self {
            Item::Highlight(h) => h.page,
            Item::Markup(m) => m.page,
        }
    }

    fn set_key(&mut self, key: Option<AnnotKey>) {
        match self {
            Item::Highlight(h) => h.key = key,
            Item::Markup(m) => m.key = key,
        }
    }
}

/// Whether a new markup can be written: one read from the file has no points
/// of its own, since the page's drawing shows it.
fn writable(m: &Markup) -> bool {
    !m.points.is_empty()
}

/// The pages whose viewports differ between two sets of scales, which are the
/// pages a save rewrites. A scale's own numbers changing counts for every page
/// using it, since the /Measure they share is written afresh.
fn scale_pages_changed(before: &ScaleStore, after: &ScaleStore) -> Vec<usize> {
    let mut pages: Vec<u32> = before.pages().chain(after.pages()).collect();
    pages.sort_unstable();
    pages.dedup();
    pages.retain(|&page| {
        let (was, now) = (before.viewports(page), after.viewports(page));
        was != now || now.iter().any(|v| before.scale(v.scale) != after.scale(v.scale))
    });
    pages.into_iter().map(|p| p as usize).collect()
}

impl Session {
    pub fn highlights(&self) -> &[HighlightEntry] {
        &self.highlights
    }

    pub fn markups(&self) -> &[MarkupEntry] {
        &self.markups
    }

    pub fn highlight(&self, uid: u64) -> Option<&HighlightEntry> {
        self.highlights.iter().find(|e| e.uid == uid)
    }

    pub fn markup(&self, uid: u64) -> Option<&MarkupEntry> {
        self.markups.iter().find(|e| e.uid == uid)
    }

    /// The document's scales and viewports. Change them with
    /// `Command::SetScales`, which undoes like any other change.
    pub fn scales(&self) -> &ScaleStore {
        &self.scales
    }

    /// Takes in the scales read from the file. Not a change, so not undoable.
    ///
    /// Reading them takes a pass over the whole file, so it happens when
    /// something first needs them; nothing may change a scale before that,
    /// or undo would step back to "no scales" and a save would then strip
    /// the file's. The app only offers the scale tools once they have
    /// arrived. If one is changed first anyway, the change stands and the
    /// steps that precede the read are forgotten, so undo can't reach a state
    /// that never was.
    pub fn load_scales(&mut self, scales: ScaleStore) {
        if self.scales == self.file_scales {
            self.scales = scales.clone();
        } else {
            self.undo.retain(|step| !matches!(step, Step::Scaled { .. }));
            self.redo.retain(|step| !matches!(step, Step::Scaled { .. }));
        }
        self.file_scales = scales;
        self.refresh();
    }

    /// Saved markups the user removed, which the pages' drawing still shows.
    pub fn erased(&self) -> &[Markup] {
        &self.erased
    }

    /// Whether there's anything to save.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    fn uid(&mut self) -> u64 {
        self.next_uid += 1;
        self.next_uid
    }

    /// Takes in highlights and markups read from the file. Not a change, so
    /// not undoable.
    ///
    /// Pages arrive a few at a time, so everything is appended and sorted once
    /// rather than inserted in place one by one. What's read from the file
    /// is saved by definition, so what's unsaved doesn't change.
    pub fn load(&mut self, highlights: Vec<Highlight>, markups: Vec<Markup>) {
        if highlights.is_empty() && markups.is_empty() {
            return;
        }
        self.highlights.reserve(highlights.len());
        for hl in highlights {
            let uid = self.uid();
            if let Some(key) = hl.key {
                self.file.insert(uid, InFile { key, comment: hl.comment.clone() });
            }
            self.highlights.push(HighlightEntry { uid, hl });
        }
        self.markups.reserve(markups.len());
        for markup in markups {
            let uid = self.uid();
            if let Some(key) = markup.key {
                self.file.insert(uid, InFile { key, comment: markup.comment.clone() });
            }
            self.markups.push(MarkupEntry { uid, markup });
        }
        // Stable, so unsaved ones keep the order they were made in.
        self.highlights.sort_by_key(|e| order(e.hl.page, e.hl.key));
        self.markups.sort_by_key(|e| order(e.markup.page, e.markup.key));
    }

    /// Puts an item among the shown ones where it sorts, after any it ties
    /// with, taking its key from the file.
    fn insert(&mut self, uid: u64, mut item: Item) {
        let key = self.file.get(&uid).map(|f| f.key);
        item.set_key(key);
        let at = order(item.page(), key);
        match item {
            Item::Highlight(hl) => {
                let i = self.highlights.partition_point(|e| order(e.hl.page, e.hl.key) <= at);
                self.highlights.insert(i, HighlightEntry { uid, hl });
            }
            Item::Markup(markup) => {
                let i = self.markups.partition_point(|e| order(e.markup.page, e.markup.key) <= at);
                self.markups.insert(i, MarkupEntry { uid, markup });
            }
        }
    }

    /// Takes a shown item out into `removed`.
    fn take(&mut self, uid: u64) -> bool {
        let item = if let Some(i) = self.highlights.iter().position(|e| e.uid == uid) {
            Item::Highlight(self.highlights.remove(i).hl)
        } else if let Some(i) = self.markups.iter().position(|e| e.uid == uid) {
            Item::Markup(self.markups.remove(i).markup)
        } else {
            return false;
        };
        self.removed.insert(uid, item);
        true
    }

    /// Puts a removed item back.
    fn restore(&mut self, uid: u64) -> bool {
        match self.removed.remove(&uid) {
            Some(item) => {
                self.insert(uid, item);
                true
            }
            None => false,
        }
    }

    fn note(&self, uid: u64) -> Option<(String, Rgb)> {
        self.highlight(uid).map(|e| (e.hl.comment.clone(), e.hl.color)).or_else(|| self.markup(uid).map(|e| (e.markup.comment.clone(), e.markup.color)))
    }

    fn set_note(&mut self, uid: u64, (comment, color): (String, Rgb)) {
        if let Some(e) = self.highlights.iter_mut().find(|e| e.uid == uid) {
            (e.hl.comment, e.hl.color) = (comment, color);
        } else if let Some(e) = self.markups.iter_mut().find(|e| e.uid == uid) {
            (e.markup.comment, e.markup.color) = (comment, color);
        }
    }

    /// Applies a command, and gives the uids of anything it added.
    pub fn apply(&mut self, command: Command) -> Vec<u64> {
        let (step, added) = match command {
            Command::AddHighlights(highlights) => {
                let uids: Vec<u64> = highlights
                    .into_iter()
                    .map(|hl| {
                        let uid = self.uid();
                        self.insert(uid, Item::Highlight(hl));
                        uid
                    })
                    .collect();
                ((!uids.is_empty()).then(|| Step::Added(uids.clone())), uids)
            }
            Command::AddMarkup(markup) => {
                let uid = self.uid();
                self.insert(uid, Item::Markup(markup));
                (Some(Step::Added(vec![uid])), vec![uid])
            }
            Command::EditNote { uid, comment, color } => {
                let step = self.note(uid).and_then(|before| {
                    let color = if self.file.contains_key(&uid) { before.1 } else { color };
                    let after = (comment, color);
                    (before != after).then(|| {
                        self.set_note(uid, after.clone());
                        Step::Edited { uid, before, after }
                    })
                });
                (step, Vec::new())
            }
            Command::Remove(uid) => (self.take(uid).then_some(Step::Removed(uid)), Vec::new()),
            Command::SetScales(scales) => {
                let step = (scales != self.scales).then(|| Step::Scaled {
                    before: Box::new(std::mem::replace(&mut self.scales, scales)),
                    after: Box::new(self.scales.clone()),
                });
                (step, Vec::new())
            }
        };
        if let Some(step) = step {
            self.undo.push_back(step);
            if self.undo.len() > HISTORY {
                self.undo.pop_front();
            }
            self.redo.clear();
            self.prune();
            self.refresh();
        }
        added
    }

    /// Undoes the last change. `false` if there was none.
    pub fn undo(&mut self) -> bool {
        let Some(step) = self.undo.pop_back() else { return false };
        match &step {
            Step::Added(uids) => uids.iter().for_each(|&uid| {
                self.take(uid);
            }),
            Step::Removed(uid) => {
                self.restore(*uid);
            }
            Step::Edited { uid, before, .. } => self.set_note(*uid, before.clone()),
            Step::Scaled { before, .. } => self.scales = (**before).clone(),
        }
        self.redo.push(step);
        self.refresh();
        true
    }

    /// Redoes the last change undone. `false` if there was none.
    pub fn redo(&mut self) -> bool {
        let Some(step) = self.redo.pop() else { return false };
        match &step {
            Step::Added(uids) => uids.iter().for_each(|&uid| {
                self.restore(uid);
            }),
            Step::Removed(uid) => {
                self.take(*uid);
            }
            Step::Edited { uid, after, .. } => self.set_note(*uid, after.clone()),
            Step::Scaled { after, .. } => self.scales = (**after).clone(),
        }
        self.undo.push_back(step);
        self.refresh();
        true
    }

    /// Forgets removed items no step can bring back and no save needs.
    fn prune(&mut self) {
        let mut wanted: HashSet<u64> = HashSet::new();
        for step in self.undo.iter().chain(&self.redo) {
            match step {
                Step::Added(uids) => wanted.extend(uids),
                Step::Removed(uid) => {
                    wanted.insert(*uid);
                }
                Step::Edited { .. } | Step::Scaled { .. } => {}
            }
        }
        let file = &self.file;
        self.removed.retain(|uid, _| wanted.contains(uid) || file.contains_key(uid));
    }

    fn new_highlights(&self) -> impl Iterator<Item = &HighlightEntry> {
        self.highlights.iter().filter(|e| e.is_new() && !e.hl.quads.is_empty())
    }

    fn new_markups(&self) -> impl Iterator<Item = &MarkupEntry> {
        self.markups.iter().filter(|e| e.markup.key.is_none() && writable(&e.markup))
    }

    /// Removed uids whose annotations are in the file.
    fn deleted(&self) -> impl Iterator<Item = (u64, AnnotKey)> + '_ {
        self.removed.keys().filter_map(|uid| self.file.get(uid).map(|f| (*uid, f.key)))
    }

    /// Keys of annotations in the file that the next save deletes.
    pub fn pending_deletes(&self) -> Vec<AnnotKey> {
        let mut keys: Vec<AnnotKey> = self.deleted().map(|(_, key)| key).collect();
        keys.sort_by_key(|k| (k.page, k.index));
        keys
    }

    fn edits(&self) -> impl Iterator<Item = (AnnotKey, &String)> {
        let file = &self.file;
        let highlights = self.highlights.iter().map(|e| (e.uid, &e.hl.comment));
        let markups = self.markups.iter().map(|e| (e.uid, &e.markup.comment));
        highlights.chain(markups).filter_map(move |(uid, comment)| file.get(&uid).filter(|f| f.comment != *comment).map(|f| (f.key, comment)))
    }

    fn refresh(&mut self) {
        self.dirty = self.new_highlights().next().is_some()
            || self.new_markups().next().is_some()
            || self.deleted().next().is_some()
            || self.edits().next().is_some()
            || self.scales != self.file_scales;
        let mut erased: Vec<Markup> = self
            .removed
            .iter()
            .filter(|(uid, _)| self.file.contains_key(uid))
            .filter_map(|(_, item)| match item {
                Item::Markup(m) => Some(m.clone()),
                Item::Highlight(_) => None,
            })
            .collect();
        erased.sort_by_key(|m| order(m.page, m.key));
        self.erased = erased;
    }

    /// Whether a save is under way.
    pub fn is_saving(&self) -> bool {
        self.saving.is_some()
    }

    /// Everything to write, marking a save as under way. `None` if there's
    /// nothing to save or a save is already running.
    pub fn begin_save(&mut self, author: String) -> Option<Changes> {
        if !self.dirty || self.saving.is_some() {
            return None;
        }
        let adds: Vec<&HighlightEntry> = self.new_highlights().collect();
        let new_markups: Vec<&MarkupEntry> = self.new_markups().collect();
        let deleted: Vec<(u64, AnnotKey)> = self.deleted().collect();
        let changes = Changes {
            adds: adds.iter().map(|e| NewHighlight { page: e.hl.page, quads: e.hl.quads.clone(), color: e.hl.color, comment: e.hl.comment.clone() }).collect(),
            markups: new_markups.iter().map(|e| e.markup.clone()).collect(),
            deletes: deleted.iter().map(|(_, key)| *key).collect(),
            edits: self.edits().map(|(key, comment)| (key, comment.clone())).collect(),
            // Only the pages whose viewports changed are written; the rest of
            // the file's /VP arrays are left alone.
            scales: (self.scales != self.file_scales).then(|| ScaleChanges {
                pages: scale_pages_changed(&self.file_scales, &self.scales),
                scales: self.scales.clone(),
            }),
            author,
        };

        let pages = changes.pages();
        let mut expected: HashMap<Group, Vec<u64>> = HashMap::new();
        let kept = |uid: &u64| !self.removed.contains_key(uid);
        // What stays in the file, by position: shown items with keys. (Every
        // shown item with a key is in `file`; removed ones are being deleted.)
        for e in self.highlights.iter().filter(|e| e.hl.key.is_some() && pages.contains(&e.hl.page) && kept(&e.uid)) {
            expected.entry((e.hl.page, false)).or_default().push(e.uid);
        }
        for e in self.markups.iter().filter(|e| e.markup.key.is_some() && pages.contains(&e.markup.page) && kept(&e.uid)) {
            expected.entry((e.markup.page, true)).or_default().push(e.uid);
        }
        // Then what's added, in the order written.
        for e in &adds {
            expected.entry((e.hl.page, false)).or_default().push(e.uid);
        }
        for e in &new_markups {
            expected.entry((e.markup.page, true)).or_default().push(e.uid);
        }
        let scales = (self.scales != self.file_scales).then(|| Box::new(self.scales.clone()));
        self.saving = Some(Saving { deleted: deleted.into_iter().map(|(uid, _)| uid).collect(), expected, scales });
        Some(changes)
    }

    /// The save failed: nothing in the file changed.
    pub fn save_failed(&mut self) {
        self.saving = None;
    }

    /// The save finished, and `highlights` and `markups` are everything now
    /// on `pages`, the pages it changed. Gives `false` if what came back
    /// didn't match what was written -- something else changed the file --
    /// in which case those pages are shown as read and undo history is
    /// cleared.
    pub fn saved(&mut self, pages: &[usize], highlights: Vec<Highlight>, markups: Vec<Markup>) -> bool {
        let Some(saving) = self.saving.take() else {
            self.replace_pages(pages, highlights, markups, &HashSet::new());
            return false;
        };
        let groups: BTreeSet<Group> = pages.iter().flat_map(|&p| [(p, false), (p, true)]).collect();
        // Each group's keys and notes as read, checked against what was
        // written before anything changes.
        let mut read: HashMap<Group, Vec<Option<(AnnotKey, String)>>> = HashMap::new();
        for h in &highlights {
            read.entry((h.page, false)).or_default().push(h.key.map(|k| (k, h.comment.clone())));
        }
        for m in &markups {
            read.entry((m.page, true)).or_default().push(m.key.map(|k| (k, m.comment.clone())));
        }
        let matches = groups.iter().all(|group| {
            let got = read.get(group).map_or(&[][..], Vec::as_slice);
            let expected = saving.expected.get(group).map_or(0, Vec::len);
            got.len() == expected && got.iter().all(Option::is_some)
        });
        if !matches {
            let written: HashSet<u64> = saving.expected.values().flatten().copied().collect();
            self.replace_pages(pages, highlights, markups, &written);
            return false;
        }

        // Every key that changes, applied in one pass over what's shown and
        // removed rather than a search per annotation.
        let mut keys: HashMap<u64, Option<AnnotKey>> = HashMap::new();
        for uid in &saving.deleted {
            self.file.remove(uid);
            // Restored since the save began: it's no longer in the file.
            keys.insert(*uid, None);
        }
        for group in &groups {
            let (Some(expected), Some(got)) = (saving.expected.get(group), read.remove(group)) else { continue };
            for (uid, (key, comment)) in expected.iter().zip(got.into_iter().flatten()) {
                self.file.insert(*uid, InFile { key, comment });
                keys.insert(*uid, Some(key));
            }
        }
        for e in &mut self.highlights {
            if let Some(key) = keys.get(&e.uid) {
                e.hl.key = *key;
            }
        }
        for e in &mut self.markups {
            if let Some(key) = keys.get(&e.uid) {
                e.markup.key = *key;
            }
        }
        for (uid, item) in &mut self.removed {
            if let Some(key) = keys.get(uid) {
                item.set_key(*key);
            }
        }
        self.highlights.sort_by_key(|e| order(e.hl.page, e.hl.key));
        self.markups.sort_by_key(|e| order(e.markup.page, e.markup.key));
        if let Some(scales) = saving.scales {
            self.file_scales = *scales;
        }
        self.prune();
        self.refresh();
        true
    }

    /// Shows `pages` exactly as `highlights` and `markups` read them. What
    /// was on them in the file, or `written` to it, goes; what was added since
    /// stays, unsaved. Undo history, which may refer to what's gone, is
    /// forgotten.
    fn replace_pages(&mut self, pages: &[usize], highlights: Vec<Highlight>, markups: Vec<Markup>, written: &HashSet<u64>) {
        let on = |page: usize| pages.contains(&page);
        let file = &self.file;
        let from_file = |uid: u64| file.contains_key(&uid) || written.contains(&uid);
        self.highlights.retain(|e| !(on(e.hl.page) && from_file(e.uid)));
        self.markups.retain(|e| !(on(e.markup.page) && from_file(e.uid)));
        self.removed.retain(|_, item| !on(item.page()));
        self.file.retain(|_, f| !on(f.key.page));
        self.undo.clear();
        self.redo.clear();
        self.load(highlights, markups);
        self.refresh();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{MarkupKind, PdfBox};

    const YELLOW: Rgb = [1.0, 0.93, 0.25];
    const BLUE: Rgb = [0.45, 0.76, 1.0];

    fn quad(y: f32) -> PdfBox {
        PdfBox { left: 72.0, bottom: y, right: 300.0, top: y + 12.0 }
    }

    fn highlight(page: usize, key: Option<usize>, comment: &str) -> Highlight {
        Highlight {
            key: key.map(|index| AnnotKey { page, index }),
            page,
            quads: vec![quad(700.0)],
            color: YELLOW,
            comment: comment.to_owned(),
            author: String::new(),
            snippet: String::new(),
        }
    }

    fn markup(page: usize, key: Option<usize>, points: bool) -> Markup {
        Markup {
            key: key.map(|index| AnnotKey { page, index }),
            page,
            kind: MarkupKind::Rectangle,
            points: if points { vec![[1.0, 1.0], [5.0, 5.0]] } else { Vec::new() },
            bounds: PdfBox { left: 1.0, bottom: 1.0, right: 5.0, top: 5.0 },
            color: [1.0, 0.0, 0.0],
            width: 1.0,
            comment: String::new(),
            author: String::new(),
        }
    }

    /// A page with two saved highlights and a saved markup read from the file.
    fn opened() -> Session {
        let mut s = Session::default();
        s.load(vec![highlight(0, Some(0), "first"), highlight(0, Some(2), "second")], vec![markup(0, Some(1), false)]);
        s
    }

    /// What the worker reads back from the file after writing `changes` to a
    /// file that held `before`, for the pages the changes touch.
    fn read_back(before: &[(usize, char, String)], changes: &Changes) -> (Vec<usize>, Vec<Highlight>, Vec<Markup>) {
        // `before` is (index, 'h' or 'm', comment) on page 0, in /Annots order.
        let mut annots: Vec<(char, String)> = before.iter().map(|(_, k, c)| (*k, c.clone())).collect();
        for (key, comment) in &changes.edits {
            annots[key.index].1.clone_from(comment);
        }
        let mut deletes: Vec<usize> = changes.deletes.iter().map(|k| k.index).collect();
        deletes.sort_unstable_by(|a, b| b.cmp(a));
        for i in deletes {
            annots.remove(i);
        }
        annots.extend(changes.adds.iter().map(|a| ('h', a.comment.clone())));
        annots.extend(changes.markups.iter().map(|m| ('m', m.comment.clone())));
        let (mut highlights, mut markups) = (Vec::new(), Vec::new());
        for (index, (kind, comment)) in annots.into_iter().enumerate() {
            if kind == 'h' {
                highlights.push(highlight(0, Some(index), &comment));
            } else {
                let mut m = markup(0, Some(index), true);
                m.comment = comment;
                markups.push(m);
            }
        }
        (vec![0], highlights, markups)
    }

    fn file_now(s: &Session) -> Vec<(usize, char, String)> {
        let is_highlight = |uid: &u64| s.highlight(*uid).is_some() || matches!(s.removed.get(uid), Some(Item::Highlight(_)));
        let mut all: Vec<(usize, char, String)> =
            s.file.iter().map(|(uid, f)| (f.key.index, if is_highlight(uid) { 'h' } else { 'm' }, f.comment.clone())).collect();
        all.sort();
        all
    }

    fn save(s: &mut Session) -> bool {
        let before = file_now(s);
        let changes = s.begin_save("me".into()).expect("something to save");
        let (pages, highlights, markups) = read_back(&before, &changes);
        s.saved(&pages, highlights, markups)
    }

    #[test]
    fn a_new_highlight_can_be_undone_and_redone_and_undoing_leaves_nothing_to_save() {
        let mut s = opened();
        assert!(!s.is_dirty() && !s.can_undo());
        let uids = s.apply(Command::AddHighlights(vec![highlight(0, None, "new")]));
        assert_eq!(uids.len(), 1);
        assert!(s.is_dirty() && s.highlight(uids[0]).unwrap().is_new());
        assert_eq!(s.highlights().last().unwrap().uid, uids[0], "unsaved ones last on the page");

        assert!(s.undo());
        assert!(s.highlight(uids[0]).is_none());
        assert!(!s.is_dirty(), "back to the file as it was");
        assert!(s.redo());
        assert_eq!(s.highlight(uids[0]).unwrap().hl.comment, "new");
        assert!(s.is_dirty());
        assert!(!s.redo());
    }

    #[test]
    fn a_new_change_clears_what_could_be_redone() {
        let mut s = opened();
        s.apply(Command::AddHighlights(vec![highlight(0, None, "a")]));
        s.undo();
        assert!(s.can_redo());
        s.apply(Command::AddMarkup(markup(0, None, true)));
        assert!(!s.can_redo());
    }

    #[test]
    fn a_saved_note_edit_keeps_its_colour_and_undoing_it_leaves_nothing_to_save() {
        let mut s = opened();
        let uid = s.highlights()[0].uid;
        s.apply(Command::EditNote { uid, comment: "changed".into(), color: BLUE });
        let e = s.highlight(uid).unwrap();
        assert_eq!((e.hl.comment.as_str(), e.hl.color), ("changed", YELLOW), "a saved colour stays");
        assert!(s.is_dirty());
        let changes = s.begin_save("me".into()).unwrap();
        assert_eq!(changes.edits, vec![(AnnotKey { page: 0, index: 0 }, "changed".to_owned())]);
        s.save_failed();
        s.undo();
        assert!(!s.is_dirty());
        // Editing back to what the file says is no change either.
        s.apply(Command::EditNote { uid, comment: "x".into(), color: YELLOW });
        s.apply(Command::EditNote { uid, comment: "first".into(), color: YELLOW });
        assert!(!s.is_dirty());
    }

    #[test]
    fn a_new_highlight_takes_a_new_colour_and_an_unchanged_note_is_no_step() {
        let mut s = opened();
        let uid = s.apply(Command::AddHighlights(vec![highlight(0, None, "")]))[0];
        s.apply(Command::EditNote { uid, comment: String::new(), color: BLUE });
        assert_eq!(s.highlight(uid).unwrap().hl.color, BLUE);
        s.apply(Command::EditNote { uid, comment: String::new(), color: BLUE });
        s.undo();
        assert_eq!(s.highlight(uid).unwrap().hl.color, YELLOW, "one step, not two");
    }

    #[test]
    fn removing_a_saved_markup_deletes_it_shows_it_crossed_out_and_undo_brings_it_back() {
        let mut s = opened();
        let uid = s.markups()[0].uid;
        s.apply(Command::Remove(uid));
        assert!(s.markup(uid).is_none());
        assert_eq!(s.erased().len(), 1);
        let changes = s.begin_save("me".into()).unwrap();
        assert_eq!(changes.deletes, vec![AnnotKey { page: 0, index: 1 }]);
        s.save_failed();
        s.undo();
        assert_eq!(s.markup(uid).unwrap().markup.key, Some(AnnotKey { page: 0, index: 1 }));
        assert!(s.erased().is_empty() && !s.is_dirty());
    }

    #[test]
    fn a_save_keeps_uids_and_undo_still_works_after_it() {
        let mut s = opened();
        let second = s.highlights()[1].uid;
        let markup_uid = s.markups()[0].uid;
        let added = s.apply(Command::AddHighlights(vec![highlight(0, None, "added")]))[0];
        s.apply(Command::Remove(markup_uid));
        assert!(save(&mut s));
        assert!(!s.is_dirty());
        // The markup at index 1 went, so the second highlight moved up, and
        // the new one follows it.
        assert_eq!(s.highlight(second).unwrap().hl.key, Some(AnnotKey { page: 0, index: 1 }));
        assert_eq!(s.highlight(added).unwrap().hl.key, Some(AnnotKey { page: 0, index: 2 }));
        assert!(s.erased().is_empty());

        // Undoing the removal after the save: it's gone from the file now, so
        // it would be added again -- but a markup read from the file has no
        // shape of its own to write, so there's nothing to save.
        s.undo();
        assert!(s.markup(markup_uid).unwrap().markup.key.is_none());
        // Undoing the new highlight after the save deletes it from the file.
        s.undo();
        assert!(s.highlight(added).is_none());
        let changes = s.begin_save("me".into()).unwrap();
        assert_eq!(changes.deletes, vec![AnnotKey { page: 0, index: 2 }]);
        assert!(changes.markups.is_empty());
    }

    #[test]
    fn changes_made_while_a_save_runs_survive_it() {
        let mut s = opened();
        let first = s.highlights()[0].uid;
        let added = s.apply(Command::AddHighlights(vec![highlight(0, None, "added")]))[0];
        let before = file_now(&s);
        let changes = s.begin_save("me".into()).unwrap();
        assert!(s.begin_save("me".into()).is_none(), "one save at a time");

        // Meanwhile: another highlight, the one being saved removed, and a
        // note changed.
        let during = s.apply(Command::AddHighlights(vec![highlight(0, None, "during")]))[0];
        s.apply(Command::Remove(added));
        s.apply(Command::EditNote { uid: first, comment: "edited during".into(), color: YELLOW });

        let (pages, highlights, markups) = read_back(&before, &changes);
        assert!(s.saved(&pages, highlights, markups));
        assert!(s.is_dirty());
        assert!(s.highlight(during).unwrap().is_new());
        let next = s.begin_save("me".into()).unwrap();
        assert_eq!(next.adds.len(), 1);
        assert_eq!(next.deletes, vec![AnnotKey { page: 0, index: 3 }], "the highlight saved, then removed");
        assert_eq!(next.edits, vec![(AnnotKey { page: 0, index: 0 }, "edited during".to_owned())]);
    }

    #[test]
    fn a_save_that_reads_back_differently_shows_the_file_and_forgets_history() {
        let mut s = opened();
        s.apply(Command::AddHighlights(vec![highlight(0, None, "added")]));
        let _ = s.begin_save("me".into()).unwrap();
        // Something else wrote the file too: one highlight more than expected.
        let read = vec![highlight(0, Some(0), "first"), highlight(0, Some(2), "second"), highlight(0, Some(3), "added"), highlight(0, Some(4), "stranger")];
        assert!(!s.saved(&[0], read, vec![markup(0, Some(1), false)]));
        assert_eq!(s.highlights().len(), 4);
        assert_eq!(s.markups().len(), 1);
        assert!(!s.is_dirty() && !s.can_undo());
    }

    fn a4() -> markup_model::Rect {
        markup_model::Rect::from_corners(markup_model::Pt::new(0.0, 0.0), markup_model::Pt::new(595.0, 842.0))
    }

    /// Scales holding page 0 at 1:`ratio`.
    fn scales_at(ratio: f64) -> (ScaleStore, markup_model::ScaleId) {
        let mut store = ScaleStore::default();
        let scale = markup_model::Scale::from_ratio(markup_model::ScaleId::new(), ratio).unwrap();
        let id = scale.id;
        store.set_scale(scale);
        store.set_page_scale(0, a4(), id);
        (store, id)
    }

    #[test]
    fn setting_a_scale_is_a_change_that_undoes_like_any_other() {
        let mut s = opened();
        assert!(!s.is_dirty());
        let (store, id) = scales_at(100.0);
        s.apply(Command::SetScales(store));
        assert!(s.is_dirty());
        assert_eq!(s.scales().page_default(0).map(|v| v.scale), Some(id));
        // Setting the same scales again is no change.
        s.apply(Command::SetScales(s.scales().clone()));
        s.undo();
        assert!(s.scales().page_default(0).is_none() && !s.is_dirty(), "one step");
        s.redo();
        assert!(s.scales().page_default(0).is_some() && s.is_dirty());
    }

    #[test]
    fn a_save_writes_the_pages_whose_scales_changed_and_remembers_them() {
        let mut s = opened();
        let (store, id) = scales_at(100.0);
        s.apply(Command::SetScales(store));
        let changes = s.begin_save("me".into()).unwrap();
        let written = changes.scales.expect("scales to write");
        assert_eq!(written.pages, vec![0]);
        assert!(s.saved(&[], Vec::new(), Vec::new()), "no annotations changed");
        assert!(!s.is_dirty(), "the file now holds them");

        // Recalibrating the same scale rewrites every page that uses it.
        let mut store = s.scales().clone();
        store.set_page_scale(3, a4(), id);
        s.apply(Command::SetScales(store));
        let mut store = s.scales().clone();
        let mut scale = store.scale(id).unwrap().clone();
        scale.metres_per_point_x *= 2.0;
        scale.metres_per_point_y *= 2.0;
        store.set_scale(scale);
        s.apply(Command::SetScales(store));
        let changes = s.begin_save("me".into()).unwrap();
        assert_eq!(changes.scales.unwrap().pages, vec![0, 3]);
        s.save_failed();
        s.undo();
        s.undo();
        assert!(!s.is_dirty(), "back to what the file holds");
    }

    /// The ratio a page measures at.
    fn ratio_of(s: &Session, page: u32) -> Option<f64> {
        let scales = s.scales();
        scales.page_default(page).and_then(|v| scales.scale(v.scale)).and_then(markup_model::Scale::ratio)
    }

    #[test]
    fn a_scale_change_undoes_to_what_the_file_holds() {
        let mut s = opened();
        let (from_file, _) = scales_at(100.0);
        s.load_scales(from_file);
        assert_eq!(ratio_of(&s, 0), Some(100.0));
        assert!(!s.is_dirty(), "the file's scales aren't a change");

        let (mine, _) = scales_at(50.0);
        s.apply(Command::SetScales(mine));
        assert_eq!(ratio_of(&s, 0), Some(50.0));
        assert!(s.is_dirty());
        s.undo();
        assert_eq!(ratio_of(&s, 0), Some(100.0), "back to the file's");
        assert!(!s.is_dirty());
    }

    #[test]
    fn a_scale_changed_before_the_file_was_read_stands_but_cannot_be_undone() {
        let mut s = opened();
        let (mine, _) = scales_at(50.0);
        s.apply(Command::SetScales(mine));
        let (from_file, _) = scales_at(100.0);
        s.load_scales(from_file);
        assert_eq!(ratio_of(&s, 0), Some(50.0), "mine stands");
        assert!(s.is_dirty(), "and still differs from the file");
        // Undoing it would step back to "no scales", which the file never
        // had, so that step is gone.
        assert!(!s.can_undo());
    }

    #[test]
    fn history_is_capped() {
        let mut s = Session::default();
        for _ in 0..HISTORY + 10 {
            s.apply(Command::AddMarkup(markup(0, None, true)));
        }
        let mut undone = 0;
        while s.undo() {
            undone += 1;
        }
        assert_eq!(undone, HISTORY);
        assert_eq!(s.markups().len(), 10);
        assert_eq!(s.removed.len(), HISTORY, "removed items kept only while a step can bring them back");
    }

    #[test]
    fn removed_items_no_step_can_reach_are_forgotten() {
        let mut s = Session::default();
        let uid = s.apply(Command::AddMarkup(markup(0, None, true)))[0];
        s.undo();
        assert!(s.removed.contains_key(&uid));
        s.apply(Command::AddMarkup(markup(0, None, true)));
        assert!(!s.removed.contains_key(&uid), "redo was cleared, so it can't come back");
    }
}

/// How long the session's work takes on a heavily marked-up document, so a
/// change to it can be checked for speed:
/// `cargo test --release --lib session::timing -- --ignored --nocapture`.
#[cfg(test)]
mod timing {
    use std::time::Instant;

    use super::*;
    use crate::model::{MarkupKind, PdfBox};

    const PAGES: usize = 500;
    const PER_PAGE: usize = 40;

    fn page(p: usize) -> (Vec<Highlight>, Vec<Markup>) {
        let highlights = (0..PER_PAGE)
            .map(|i| Highlight {
                key: Some(AnnotKey { page: p, index: i * 2 }),
                page: p,
                quads: vec![PdfBox { left: 72.0, bottom: i as f32 * 14.0, right: 400.0, top: i as f32 * 14.0 + 12.0 }],
                color: [1.0, 0.93, 0.25],
                comment: format!("note {i}"),
                author: "someone".into(),
                snippet: "some quoted words from the page".into(),
            })
            .collect();
        let markups = (0..PER_PAGE)
            .map(|i| Markup {
                key: Some(AnnotKey { page: p, index: i * 2 + 1 }),
                page: p,
                kind: MarkupKind::Rectangle,
                points: Vec::new(),
                bounds: PdfBox { left: 1.0, bottom: 1.0, right: 5.0, top: 5.0 },
                color: [1.0, 0.0, 0.0],
                width: 1.0,
                comment: String::new(),
                author: String::new(),
            })
            .collect();
        (highlights, markups)
    }

    #[test]
    #[ignore]
    fn timing() {
        let pages: Vec<_> = (0..PAGES).map(page).collect();
        let mut s = Session::default();
        let started = Instant::now();
        // Pages arrive one at a time, out of order, as they're viewed and scanned.
        for (highlights, markups) in pages.iter().rev().cloned() {
            s.load(highlights, markups);
        }
        let load = started.elapsed();
        let total = PAGES * PER_PAGE * 2;

        let started = Instant::now();
        let target = s.highlights()[total / 4].uid;
        for i in 0..100 {
            let mut hl = s.highlights()[0].hl.clone();
            hl.key = None;
            hl.page = i % PAGES;
            s.apply(Command::AddHighlights(vec![hl]));
            s.apply(Command::EditNote { uid: target, comment: format!("edit {i}"), color: [0.0; 3] });
        }
        let per_command = started.elapsed() / 200;

        let started = Instant::now();
        for _ in 0..50 {
            s.undo();
        }
        let per_undo = started.elapsed() / 50;

        let started = Instant::now();
        let changes = s.begin_save("me".into()).unwrap();
        let begin = started.elapsed();
        // What the worker reads back for the pages changed.
        let pages_changed: Vec<usize> = changes.pages().into_iter().collect();
        let (mut highlights, mut markups) = (Vec::new(), Vec::new());
        for &p in &pages_changed {
            let (h, m) = page(p);
            let added = changes.adds.iter().filter(|a| a.page == p).count();
            let edited = changes.edits.iter().filter(|(k, _)| k.page == p);
            let mut h = h;
            for (key, comment) in edited {
                if let Some(x) = h.iter_mut().find(|x| x.key == Some(*key)) {
                    x.comment.clone_from(comment);
                }
            }
            for j in 0..added {
                let mut extra = h[0].clone();
                extra.key = Some(AnnotKey { page: p, index: PER_PAGE * 2 + j });
                h.push(extra);
            }
            highlights.extend(h);
            markups.extend(m);
        }
        let started = Instant::now();
        let matched = s.saved(&pages_changed, highlights, markups);
        let saved = started.elapsed();
        assert!(matched);
        println!(
            "{total} annotations over {PAGES} pages: load {load:?}; per command {per_command:?}; per undo {per_undo:?}; begin save {begin:?}; save read back ({} pages) {saved:?}",
            pages_changed.len()
        );
    }
}
