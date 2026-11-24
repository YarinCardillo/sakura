//! The engine's own `flow changes` output for a made-up lane (made by its own code in a throwaway home): the app
//! must read the list and the patch of one file.

use sakura_core::engine::changes::{ChangesPage, FilePatch};

#[test]
fn a_real_changes_listing_parses_with_its_files_and_totals() {
    let page: ChangesPage =
        serde_json::from_str(include_str!("fixtures/engine-real-changes.json")).expect("a listing");
    assert!(!page.gone);
    assert_eq!(page.files.len(), 3);
    assert_eq!((page.added, page.removed), (Some(20), Some(3)));
    assert_eq!(page.files[2].status, "added");
}

#[test]
fn a_real_patch_parses_with_its_text() {
    let patch: FilePatch =
        serde_json::from_str(include_str!("fixtures/engine-real-changes-file.json"))
            .expect("a patch");
    assert!(patch
        .patch
        .starts_with("diff --git a/src/upload/uploadQueue.ts"));
    assert!(!patch.truncated);
}
