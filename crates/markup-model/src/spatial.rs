//! Markups on each page by their bounds, in an R-tree: which are in view, and
//! which might be under the pointer, without looking at every markup.

use std::collections::HashMap;

use rstar::primitives::{GeomWithData, Rectangle};
use rstar::{RTree, AABB};

use crate::geom::{Pt, Rect};
use crate::id::{MarkupId, PageIndex};

type Entry = GeomWithData<Rectangle<[f64; 2]>, MarkupId>;

fn entry(bounds: Rect, id: MarkupId) -> Entry {
    GeomWithData::new(Rectangle::from_corners([bounds.min.x, bounds.min.y], [bounds.max.x, bounds.max.y]), id)
}

fn envelope(r: Rect) -> AABB<[f64; 2]> {
    AABB::from_corners([r.min.x, r.min.y], [r.max.x, r.max.y])
}

#[derive(Debug, Default)]
pub struct SpatialIndex {
    pages: HashMap<PageIndex, RTree<Entry>>,
    /// Where each markup is filed, to find its entry again.
    filed: HashMap<MarkupId, (PageIndex, Rect)>,
}

impl SpatialIndex {
    /// An index of many markups at once, packed better and built faster than
    /// inserting them one by one: for opening a file.
    pub fn bulk(items: impl IntoIterator<Item = (MarkupId, PageIndex, Rect)>) -> SpatialIndex {
        let mut by_page: HashMap<PageIndex, Vec<Entry>> = HashMap::new();
        let mut filed = HashMap::new();
        for (id, page, bounds) in items {
            // A later entry for the same markup replaces the earlier.
            if let Some((old_page, old)) = filed.insert(id, (page, bounds)) {
                if let Some(v) = by_page.get_mut(&old_page) {
                    v.retain(|e| *e != entry(old, id));
                }
            }
            by_page.entry(page).or_default().push(entry(bounds, id));
        }
        SpatialIndex { pages: by_page.into_iter().map(|(p, v)| (p, RTree::bulk_load(v))).collect(), filed }
    }

    /// Files a markup, or moves it if it's already filed.
    pub fn set(&mut self, id: MarkupId, page: PageIndex, bounds: Rect) {
        if self.filed.get(&id) == Some(&(page, bounds)) {
            return;
        }
        self.remove(id);
        self.pages.entry(page).or_default().insert(entry(bounds, id));
        self.filed.insert(id, (page, bounds));
    }

    pub fn remove(&mut self, id: MarkupId) {
        if let Some((page, bounds)) = self.filed.remove(&id) {
            if let Some(tree) = self.pages.get_mut(&page) {
                tree.remove(&entry(bounds, id));
            }
        }
    }

    /// Markups on `page` whose bounds meet `area`.
    pub fn in_rect(&self, page: PageIndex, area: Rect) -> impl Iterator<Item = MarkupId> + '_ {
        self.pages.get(&page).into_iter().flat_map(move |t| t.locate_in_envelope_intersecting(envelope(area)).map(|e| e.data))
    }

    /// Markups on `page` whose bounds come within `reach` of `p`.
    pub fn near(&self, page: PageIndex, p: Pt, reach: f64) -> impl Iterator<Item = MarkupId> + '_ {
        self.in_rect(page, Rect::from_corners(p, p).expand(reach))
    }

    pub fn len(&self) -> usize {
        self.filed.len()
    }

    pub fn is_empty(&self) -> bool {
        self.filed.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x0: f64, y0: f64, x1: f64, y1: f64) -> Rect {
        Rect::from_corners(Pt::new(x0, y0), Pt::new(x1, y1))
    }

    #[test]
    fn finds_what_is_in_view_on_that_page_only() {
        let (a, b, c) = (MarkupId(1), MarkupId(2), MarkupId(3));
        let mut index = SpatialIndex::bulk([(a, 0, r(0.0, 0.0, 10.0, 10.0)), (b, 0, r(100.0, 100.0, 110.0, 110.0)), (c, 1, r(0.0, 0.0, 10.0, 10.0))]);
        let mut found: Vec<_> = index.in_rect(0, r(5.0, 5.0, 50.0, 50.0)).collect();
        found.sort();
        assert_eq!(found, [a]);
        assert_eq!(index.near(0, Pt::new(98.0, 98.0), 3.0).collect::<Vec<_>>(), [b]);

        index.set(a, 0, r(200.0, 200.0, 210.0, 210.0));
        assert_eq!(index.in_rect(0, r(5.0, 5.0, 50.0, 50.0)).count(), 0, "moved away");
        index.set(b, 1, r(0.0, 0.0, 1.0, 1.0));
        assert_eq!(index.in_rect(0, r(0.0, 0.0, 1000.0, 1000.0)).collect::<Vec<_>>(), [a], "moved to another page");
        index.remove(c);
        assert_eq!(index.in_rect(1, r(0.0, 0.0, 1000.0, 1000.0)).collect::<Vec<_>>(), [b]);
        assert_eq!(index.len(), 2);
    }
}
