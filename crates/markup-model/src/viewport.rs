//! Which scale a measurement uses.
//!
//! Every page has a list of viewports, each a box with a scale, as a /VP
//! array holds. At launch a page has at most one, covering the whole page, but
//! nothing reads "the page's scale" directly: every quantity resolves its
//! scale through `ScaleStore::resolve`, so regions with their own scales
//! (a detail drawn at 1:20 on a 1:100 sheet) need no change here.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::geom::{Pt, Rect};
use crate::id::{PageIndex, ScaleId, ViewportId};
use crate::scale::Scale;

/// A region of a page with its own scale: a /VP entry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Viewport {
    pub id: ViewportId,
    pub page: PageIndex,
    /// In user space.
    pub bbox: Rect,
    /// "Detail A". Empty for the whole-page viewport.
    pub name: String,
    pub scale: ScaleId,
    /// The page's default: used for measurements outside every other
    /// viewport, including any drawn off the page's edge.
    pub whole_page: bool,
}

/// How a markup finds its scale.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ScaleRef {
    /// From the page's viewports, by where the markup's first point is.
    #[default]
    Page,
    /// This scale, whatever the page says.
    Override(ScaleId),
}

/// Every scale in a document, and every page's viewports.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ScaleStore {
    scales: BTreeMap<ScaleId, Scale>,
    /// In /VP order: later entries win where viewports overlap.
    pages: BTreeMap<PageIndex, Vec<Viewport>>,
}

impl ScaleStore {
    pub fn scale(&self, id: ScaleId) -> Option<&Scale> {
        self.scales.get(&id)
    }

    pub fn scales(&self) -> impl Iterator<Item = &Scale> {
        self.scales.values()
    }

    /// Adds or replaces a scale, giving back the one it replaced.
    pub fn set_scale(&mut self, scale: Scale) -> Option<Scale> {
        self.scales.insert(scale.id, scale)
    }

    /// Removes a scale that nothing uses any more. Refuses, giving `false`,
    /// while a viewport still points at it.
    pub fn remove_scale(&mut self, id: ScaleId) -> bool {
        if self.viewports_all().any(|v| v.scale == id) {
            return false;
        }
        self.scales.remove(&id).is_some()
    }

    /// An existing scale that measures the same as `scale`, for merging the
    /// copies of one /Measure that other programs write per annotation.
    pub fn find_same(&self, scale: &Scale) -> Option<ScaleId> {
        self.scales.values().find(|s| s.same_measure(scale)).map(|s| s.id)
    }

    pub fn viewports(&self, page: PageIndex) -> &[Viewport] {
        self.pages.get(&page).map_or(&[], Vec::as_slice)
    }

    fn viewports_all(&self) -> impl Iterator<Item = &Viewport> {
        self.pages.values().flatten()
    }

    pub fn viewport(&self, id: ViewportId) -> Option<&Viewport> {
        self.viewports_all().find(|v| v.id == id)
    }

    /// Adds a viewport at the end of its page's list, or replaces the one
    /// with its ID where it stands. Gives back the one replaced.
    pub fn set_viewport(&mut self, viewport: Viewport) -> Option<Viewport> {
        let list = self.pages.entry(viewport.page).or_default();
        match list.iter_mut().find(|v| v.id == viewport.id) {
            Some(existing) => Some(std::mem::replace(existing, viewport)),
            None => {
                list.push(viewport);
                None
            }
        }
    }

    /// Removes a viewport, giving it back with where it was in its page's
    /// list, so undo can put it back in the same place.
    pub fn remove_viewport(&mut self, id: ViewportId) -> Option<(Viewport, usize)> {
        for list in self.pages.values_mut() {
            if let Some(at) = list.iter().position(|v| v.id == id) {
                return Some((list.remove(at), at));
            }
        }
        None
    }

    /// Puts a removed viewport back at `at` in its page's list.
    pub fn insert_viewport(&mut self, viewport: Viewport, at: usize) {
        let list = self.pages.entry(viewport.page).or_default();
        list.insert(at.min(list.len()), viewport);
    }

    /// The page's whole-page viewport.
    pub fn page_default(&self, page: PageIndex) -> Option<&Viewport> {
        self.viewports(page).iter().rev().find(|v| v.whole_page)
    }

    /// Sets the scale of the whole of `page`, whose box is `page_box`,
    /// reusing its whole-page viewport if it has one. Gives back the viewport
    /// as it was before, if there was one, for undo.
    pub fn set_page_scale(&mut self, page: PageIndex, page_box: Rect, scale: ScaleId) -> Option<Viewport> {
        let id = self.page_default(page).map_or_else(ViewportId::new, |v| v.id);
        self.set_viewport(Viewport { id, page, bbox: page_box, name: String::new(), scale, whole_page: true })
    }

    /// The scale for a markup on `page` whose first point is `first`, and the
    /// viewport it came from, if any. In order: an override on the markup;
    /// the last viewport in the page's list whose box holds the point (as
    /// ISO 32000 says for /VP); the page's whole-page viewport; none, and the
    /// measurement is uncalibrated.
    pub fn resolve(&self, page: PageIndex, first: Option<Pt>, scale_ref: ScaleRef) -> Option<(&Scale, Option<ViewportId>)> {
        if let ScaleRef::Override(id) = scale_ref {
            return self.scale(id).map(|s| (s, None));
        }
        let viewports = self.viewports(page);
        let inside = first.and_then(|p| viewports.iter().rev().find(|v| v.bbox.contains(p)));
        let viewport = inside.or_else(|| self.page_default(page))?;
        self.scale(viewport.scale).map(|s| (s, Some(viewport.id)))
    }

    /// Gives every page in `pages` the whole-page scale of page `from`, the
    /// same scale rather than a copy, so recalibrating one recalibrates all.
    /// `page_box` gives each target page's box. Returns each target's
    /// whole-page viewport as it was, for undo; `None` if `from` has no
    /// whole-page scale.
    pub fn copy_page_scale(
        &mut self,
        from: PageIndex,
        pages: impl IntoIterator<Item = PageIndex>,
        page_box: impl Fn(PageIndex) -> Rect,
    ) -> Option<Vec<(PageIndex, Option<Viewport>)>> {
        let scale = self.page_default(from)?.scale;
        Some(pages.into_iter().filter(|&p| p != from).map(|p| (p, self.set_page_scale(p, page_box(p), scale))).collect())
    }

    /// Every page that has viewports.
    pub fn pages(&self) -> impl Iterator<Item = PageIndex> + '_ {
        self.pages.keys().copied()
    }

    /// Pages with a viewport using `scale`.
    pub fn pages_using(&self, scale: ScaleId) -> Vec<PageIndex> {
        self.pages.iter().filter(|(_, list)| list.iter().any(|v| v.scale == scale)).map(|(&p, _)| p).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scale::Scale;

    fn page_box() -> Rect {
        Rect::from_corners(Pt::new(0.0, 0.0), Pt::new(3370.0, 2384.0))
    }

    fn store_with(ratios: &[f64]) -> (ScaleStore, Vec<ScaleId>) {
        let mut store = ScaleStore::default();
        let ids = ratios
            .iter()
            .map(|&r| {
                let s = Scale::from_ratio(ScaleId::new(), r).unwrap();
                let id = s.id;
                store.set_scale(s);
                id
            })
            .collect();
        (store, ids)
    }

    fn detail(page: PageIndex, bbox: Rect, scale: ScaleId) -> Viewport {
        Viewport { id: ViewportId::new(), page, bbox, name: "Detail A".into(), scale, whole_page: false }
    }

    #[test]
    fn a_page_with_no_scale_is_uncalibrated() {
        let (store, _) = store_with(&[100.0]);
        assert!(store.resolve(0, Some(Pt::new(10.0, 10.0)), ScaleRef::Page).is_none());
    }

    #[test]
    fn a_detail_region_beats_the_page_and_the_last_region_wins() {
        let (mut store, ids) = store_with(&[100.0, 20.0, 5.0]);
        store.set_page_scale(0, page_box(), ids[0]);
        store.set_viewport(detail(0, Rect::from_corners(Pt::new(100.0, 100.0), Pt::new(500.0, 500.0)), ids[1]));
        store.set_viewport(detail(0, Rect::from_corners(Pt::new(400.0, 400.0), Pt::new(600.0, 600.0)), ids[2]));
        let ratio = |p: Pt| store.resolve(0, Some(p), ScaleRef::Page).unwrap().0.ratio().unwrap().round();
        assert_eq!(ratio(Pt::new(50.0, 50.0)), 100.0, "the page");
        assert_eq!(ratio(Pt::new(200.0, 200.0)), 20.0, "detail A");
        assert_eq!(ratio(Pt::new(450.0, 450.0)), 5.0, "in both: the later one");
        assert_eq!(ratio(Pt::new(-50.0, 50.0)), 100.0, "off the page's edge: the page's default");
    }

    #[test]
    fn an_override_beats_every_viewport() {
        let (mut store, ids) = store_with(&[100.0, 20.0]);
        store.set_page_scale(0, page_box(), ids[0]);
        let (s, from) = store.resolve(0, Some(Pt::new(5.0, 5.0)), ScaleRef::Override(ids[1])).unwrap();
        assert_eq!((s.id, from), (ids[1], None));
    }

    #[test]
    fn setting_a_page_scale_again_reuses_its_viewport() {
        let (mut store, ids) = store_with(&[100.0, 200.0]);
        assert!(store.set_page_scale(0, page_box(), ids[0]).is_none());
        let before = store.set_page_scale(0, page_box(), ids[1]).unwrap();
        assert_eq!(before.scale, ids[0]);
        assert_eq!(store.viewports(0).len(), 1);
        assert_eq!(store.page_default(0).unwrap().id, before.id);
    }

    #[test]
    fn a_copied_scale_is_shared_not_duplicated() {
        let (mut store, ids) = store_with(&[250.0]);
        store.set_page_scale(3, page_box(), ids[0]);
        let undo = store.copy_page_scale(3, 0..6, |_| page_box()).unwrap();
        assert_eq!(undo.len(), 5, "every page but the source");
        assert_eq!(store.pages_using(ids[0]), vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(store.scales().count(), 1);
        assert!(!store.remove_scale(ids[0]), "still in use");
    }

    #[test]
    fn a_removed_viewport_goes_back_where_it_was() {
        let (mut store, ids) = store_with(&[100.0]);
        let a = detail(0, page_box(), ids[0]);
        let b = detail(0, page_box(), ids[0]);
        let (a_id, b_id) = (a.id, b.id);
        store.set_viewport(a);
        store.set_viewport(b);
        let (removed, at) = store.remove_viewport(a_id).unwrap();
        assert_eq!(at, 0);
        store.insert_viewport(removed, at);
        assert_eq!(store.viewports(0).iter().map(|v| v.id).collect::<Vec<_>>(), [a_id, b_id]);
    }
}
