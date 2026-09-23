mod common;

use std::sync::Arc;
use common::{build_pdf, next_reply, scratch_dir, start_worker_with};
use kinetic_pdf::{arrange::Arrangement, cache::{self, Cache, Key}, model::{Changes, Reply, Request}, pool::Helpers};

#[test]
fn saving_a_rotation_and_two_page_move_does_not_reuse_the_old_page_cache() {
    let dir = scratch_dir("arrange-cache");
    let path = dir.join("document.pdf");
    let bytes = build_pdf(4, &[]);
    std::fs::write(&path, &bytes).unwrap();
    let before = cache::fingerprint(&bytes);
    let cache = Arc::new(Cache::open(dir.join("pages"), cache::DEFAULT_LIMIT).unwrap());
    for page in 0..4 {
        cache.store(Key::new(before, page, 1.0), [1, 1], vec![page as u8, 0, 0, 255]);
        cache.store(Key::thumbnail(before, page), [1, 1], vec![page as u8, 0, 0, 255]);
        cache.store_shapes(before, page, 1.0, vec![page as u8]);
    }
    std::fs::write(cache.copy_path(before), &bytes).unwrap();
    cache.adopt_copy(before);
    cache.flush();
    let (tx, rx, _, _) = start_worker_with(Helpers::none(), Some(cache.clone()));
    tx.send(Request::Open { generation: 1, path: path.clone() }).unwrap();
    loop { if matches!(next_reply(&rx), Reply::Opened { .. }) { break; } }
    let mut order = Arrangement::new(4);
    order.click(0, false, false);
    order.rotate(1);
    order.click(1, true, false);
    order.move_selected(4);
    tx.send(Request::Save { generation: 1, changes: Changes::default(), arrangement: Some(order.sheets().to_vec()) }).unwrap();
    loop {
        match next_reply(&rx) {
            Reply::Saved { .. } => break,
            Reply::SaveFailed { error, .. } => panic!("{error}"),
            _ => {}
        }
    }
    let after = cache::fingerprint(&std::fs::read(&path).unwrap());
    assert_ne!(before, after);
    for page in 0..4 {
        assert!(!cache.has_image(Key::new(after, page, 1.0)), "old page {page} image was assigned to a different sheet");
        assert!(!cache.has_image(Key::thumbnail(after, page)), "old thumbnail survived reorder");
        assert!(!cache.has_shapes(after, page, 1.0), "old GPU shapes survived reorder");
    }
    assert!(cache.copy(after).is_none(), "helpers would draw the old PDF copy");
}
