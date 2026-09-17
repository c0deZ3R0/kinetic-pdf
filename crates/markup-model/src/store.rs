//! Every markup in a document, the one place they're held, with each one's
//! quantities kept up to date as its geometry or scale changes.

use std::collections::HashMap;

use crate::geom::{Pt, Rect};
use crate::hit::Hit;
use crate::id::{MarkupId, PageIndex, ScaleId, ViewportId};
use crate::markup::{Geometry, Markup};
use crate::quantity::{self, Quantities, QuantityError, Totals};
use crate::scale::Scale;
use crate::spatial::SpatialIndex;
use crate::viewport::ScaleStore;

/// A markup's quantities as last worked out, and the scale they used.
#[derive(Clone, Debug, PartialEq)]
pub struct Measured {
    pub scale: Option<ScaleId>,
    pub viewport: Option<ViewportId>,
    pub result: Result<Quantities, QuantityError>,
}

fn measure(m: &Markup, scales: &ScaleStore) -> Measured {
    let resolved = scales.resolve(m.page, m.geometry.first_point(), m.scale_ref);
    Measured {
        scale: resolved.map(|(s, _)| s.id),
        viewport: resolved.and_then(|(_, v)| v),
        result: quantity::quantities(m, resolved.map(|(s, _)| s)),
    }
}

/// What recalibrating a scale would do, to show before it's done.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RescaleSummary {
    pub markups: usize,
    pub before: Totals,
    pub after: Totals,
}

struct Entry {
    markup: Markup,
    measured: Measured,
}

#[derive(Default)]
pub struct MarkupStore {
    entries: HashMap<MarkupId, Entry>,
    index: SpatialIndex,
}

impl MarkupStore {
    /// A store of many markups at once, as read from a file.
    pub fn bulk(markups: impl IntoIterator<Item = Markup>, scales: &ScaleStore) -> MarkupStore {
        let entries: HashMap<MarkupId, Entry> =
            markups.into_iter().map(|m| (m.id, Entry { measured: measure(&m, scales), markup: m })).collect();
        let index = SpatialIndex::bulk(entries.values().filter_map(|e| Some((e.markup.id, e.markup.page, e.markup.geometry.bounds()?))));
        MarkupStore { entries, index }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, id: MarkupId) -> Option<&Markup> {
        self.entries.get(&id).map(|e| &e.markup)
    }

    pub fn measured(&self, id: MarkupId) -> Option<&Measured> {
        self.entries.get(&id).map(|e| &e.measured)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Markup, &Measured)> {
        self.entries.values().map(|e| (&e.markup, &e.measured))
    }

    fn file(&mut self, id: MarkupId) {
        let Some(e) = self.entries.get(&id) else { return };
        match e.markup.geometry.bounds() {
            Some(bounds) => self.index.set(id, e.markup.page, bounds),
            None => self.index.remove(id),
        }
    }

    /// Adds a markup, or replaces the one with its ID, giving that back.
    pub fn insert(&mut self, markup: Markup, scales: &ScaleStore) -> Option<Markup> {
        let id = markup.id;
        let measured = measure(&markup, scales);
        let old = self.entries.insert(id, Entry { markup, measured }).map(|e| e.markup);
        self.file(id);
        old
    }

    pub fn remove(&mut self, id: MarkupId) -> Option<Markup> {
        self.index.remove(id);
        self.entries.remove(&id).map(|e| e.markup)
    }

    /// Replaces a markup's geometry and measures it again: the one markup, so
    /// dragging a vertex stays quick however many others there are. Gives
    /// back the geometry it had.
    pub fn set_geometry(&mut self, id: MarkupId, geometry: Geometry, scales: &ScaleStore) -> Option<Geometry> {
        self.update(id, scales, |m| std::mem::replace(&mut m.geometry, geometry))
    }

    /// Changes a markup with `change` and measures it again, giving back what
    /// `change` returns.
    pub fn update<R>(&mut self, id: MarkupId, scales: &ScaleStore, change: impl FnOnce(&mut Markup) -> R) -> Option<R> {
        let e = self.entries.get_mut(&id)?;
        let out = change(&mut e.markup);
        e.measured = measure(&e.markup, scales);
        self.file(id);
        Some(out)
    }

    /// Measures again every markup that used `scale` or now resolves to it,
    /// after it was recalibrated. Gives how many were.
    pub fn rescaled(&mut self, scale: ScaleId, scales: &ScaleStore) -> usize {
        let mut count = 0;
        for e in self.entries.values_mut() {
            let now = measure(&e.markup, scales);
            if e.measured.scale == Some(scale) || now.scale == Some(scale) {
                e.measured = now;
                count += 1;
            }
        }
        count
    }

    /// Measures again every markup on `page`, after its viewports changed.
    pub fn page_rescaled(&mut self, page: PageIndex, scales: &ScaleStore) -> usize {
        let mut count = 0;
        for e in self.entries.values_mut().filter(|e| e.markup.page == page) {
            e.measured = measure(&e.markup, scales);
            count += 1;
        }
        count
    }

    /// What replacing the scale with `new`'s ID by `new` would change: the
    /// markups using it, and their totals before and after. Changes nothing.
    pub fn preview_rescale(&self, new: &Scale) -> RescaleSummary {
        let mut summary = RescaleSummary::default();
        for e in self.entries.values().filter(|e| e.measured.scale == Some(new.id)) {
            summary.markups += 1;
            summary.before.add(&e.measured.result);
            summary.after.add(&quantity::quantities(&e.markup, Some(new)));
        }
        summary
    }

    /// Totals of the markups `include` picks.
    pub fn totals(&self, mut include: impl FnMut(&Markup) -> bool) -> Totals {
        self.entries.values().filter(|e| include(&e.markup)).map(|e| &e.measured.result).collect()
    }

    /// Markups on `page` whose bounds meet `area`.
    pub fn in_rect(&self, page: PageIndex, area: Rect) -> impl Iterator<Item = &Markup> + '_ {
        self.index.in_rect(page, area).filter_map(|id| self.get(id))
    }

    /// The markup part at `p` on `page` within `tolerance` points: vertices
    /// before midpoints before edges before insides, and among insides the
    /// smallest markup, so one drawn inside another can still be picked.
    pub fn pick(&self, page: PageIndex, p: Pt, tolerance: f64) -> Option<(MarkupId, Hit)> {
        let rank = |h: &Hit| match h {
            Hit::Vertex { .. } => 0,
            Hit::Midpoint { .. } => 1,
            Hit::Edge { .. } => 2,
            Hit::Inside => 3,
        };
        self.index
            .near(page, p, tolerance)
            .filter_map(|id| {
                let m = self.get(id)?;
                let hit = m.kind.measure().hit_test(m, p, tolerance)?;
                let size = m.geometry.bounds().map_or(0.0, |b| b.area());
                Some((rank(&hit), size, id, hit))
            })
            .min_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)).then(a.2.cmp(&b.2)))
            .map(|(_, _, id, hit)| (id, hit))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markup::MarkupKind;

    fn page_box() -> Rect {
        Rect::from_corners(Pt::new(0.0, 0.0), Pt::new(1000.0, 1000.0))
    }

    fn square(x: f64, size: f64) -> Markup {
        let pts = vec![Pt::new(x, x), Pt::new(x + size, x), Pt::new(x + size, x + size), Pt::new(x, x + size)];
        Markup::new(0, MarkupKind::Area, Geometry::Polygon { pts, holes: vec![] })
    }

    fn scales(mpp: f64) -> (ScaleStore, ScaleId) {
        let mut scales = ScaleStore::default();
        let s = Scale::uniform(ScaleId::new(), mpp, "test").unwrap();
        let id = s.id;
        scales.set_scale(s);
        scales.set_page_scale(0, page_box(), id);
        (scales, id)
    }

    #[test]
    fn quantities_follow_geometry_changes() {
        let (scales, id) = scales(1.0);
        let mut store = MarkupStore::default();
        let m = square(0.0, 10.0);
        let mid = m.id;
        store.insert(m, &scales);
        assert_eq!(store.measured(mid).unwrap().result.unwrap().area_m2, Some(100.0));
        assert_eq!(store.measured(mid).unwrap().scale, Some(id));
        let old = store.set_geometry(mid, square(0.0, 20.0).geometry, &scales).unwrap();
        assert_eq!(store.measured(mid).unwrap().result.unwrap().area_m2, Some(400.0));
        store.set_geometry(mid, old, &scales);
        assert_eq!(store.measured(mid).unwrap().result.unwrap().area_m2, Some(100.0), "undone");
    }

    #[test]
    fn recalibrating_previews_then_updates_every_markup_using_the_scale() {
        let (mut scales, id) = scales(1.0);
        let store_markups: Vec<Markup> = (0..42).map(|i| square(f64::from(i) * 20.0, 10.0)).collect();
        let mut store = MarkupStore::bulk(store_markups, &scales);
        let mut new = scales.scale(id).unwrap().clone();
        new.metres_per_point_x = 2.0;
        new.metres_per_point_y = 2.0;
        let summary = store.preview_rescale(&new);
        assert_eq!(summary.markups, 42);
        assert_eq!((summary.before.area_m2, summary.after.area_m2), (4200.0, 16800.0));
        assert_eq!(store.totals(|_| true).area_m2, 4200.0, "a preview changes nothing");

        let old = scales.set_scale(new).unwrap();
        assert_eq!(store.rescaled(id, &scales), 42);
        assert_eq!(store.totals(|_| true).area_m2, 16800.0);
        scales.set_scale(old);
        store.rescaled(id, &scales);
        assert_eq!(store.totals(|_| true).area_m2, 4200.0, "undone");
    }

    #[test]
    fn markups_on_an_uncalibrated_page_are_left_out_of_totals() {
        let mut store = MarkupStore::default();
        store.insert(square(0.0, 10.0), &ScaleStore::default());
        let totals = store.totals(|_| true);
        assert_eq!((totals.included, totals.excluded, totals.area_m2), (0, 1, 0.0));
        assert!(store.iter().all(|(_, m)| m.result.unwrap().uncalibrated));
    }

    #[test]
    fn picking_prefers_handles_then_the_smaller_shape() {
        let (scales, _) = scales(1.0);
        let big = square(0.0, 100.0);
        let small = square(40.0, 10.0);
        let (big_id, small_id) = (big.id, small.id);
        let store = MarkupStore::bulk([big, small], &scales);
        assert_eq!(store.pick(0, Pt::new(45.0, 45.0), 2.0), Some((small_id, Hit::Inside)));
        assert_eq!(store.pick(0, Pt::new(20.0, 20.0), 2.0), Some((big_id, Hit::Inside)));
        assert_eq!(store.pick(0, Pt::new(100.5, 99.0), 2.0), Some((big_id, Hit::Vertex { ring: 0, index: 2 })));
        assert_eq!(store.pick(0, Pt::new(500.0, 500.0), 2.0), None);
        assert_eq!(store.in_rect(0, Rect::from_corners(Pt::new(45.0, 45.0), Pt::new(46.0, 46.0))).count(), 2);
    }
}
