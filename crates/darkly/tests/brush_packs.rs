//! End-to-end coverage for brush packs through a real `DarklyEngine`.
//!
//! The library is process-global, so each test resets it first — otherwise one
//! test's packs leak into the next within this binary.

use darkly::brush::library;
use darkly::brush::pack::PackPalette;
use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::test_device;

/// A shape-valid palette, for the tests that do not care which colors a pack
/// wears. One fixture rather than four literals at every call site.
fn palette() -> PackPalette {
    PackPalette::new("#2f7fe0", "#2fd0c0", "#0c1a26", "#c3dae9")
}

fn fresh_engine() -> DarklyEngine {
    library::reset_for_test();
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, 1024, 768)
}

#[test]
fn library_list_reports_every_shipped_pack_with_its_members() {
    let engine = fresh_engine();
    let snap = engine.library_list();

    assert!(!snap.brushes.is_empty(), "shipped brushes are listed");
    let ids: Vec<&str> = snap.packs.iter().map(|p| p.id.as_str()).collect();
    for expected in ["basic", "dry_media", "wet_media", "effects", "misc"] {
        assert!(ids.contains(&expected), "pack '{expected}' is listed");
    }

    // Every member id resolves to a brush in the same snapshot.
    for pack in &snap.packs {
        for member in &pack.members {
            assert!(
                snap.brushes.iter().any(|b| &b.id == member),
                "pack '{}' names '{member}', absent from the same snapshot",
                pack.id
            );
        }
    }

    let basic = snap.packs.iter().find(|p| p.id == "basic").unwrap();
    assert!(basic.members.contains(&"ink_pen".to_string()));
}

#[test]
fn pack_info_reports_permissions_matching_the_pack() {
    let engine = fresh_engine();
    let snap = engine.library_list();

    let basic = snap.packs.iter().find(|p| p.id == "basic").unwrap();
    assert!(!basic.can_edit_members, "a shipped pack is fixed");
    assert!(!basic.can_edit_identity);
}

#[test]
fn brush_save_then_library_list_shows_it() {
    let mut engine = fresh_engine();
    engine.brush_save("my_brush", "My Brush").unwrap();

    let snap = engine.library_list();
    let saved = snap
        .brushes
        .iter()
        .find(|b| b.id == "my_brush")
        .expect("the saved brush is listed");
    assert_eq!(saved.name, "My Brush");

    // Saved brushes belong to no pack until the painter puts them in one, and
    // that is a reachable, safe state.
    assert!(!snap.packs.iter().any(|p| p.members.contains(&saved.id)));
}

#[test]
fn brush_info_reports_who_may_edit_the_brush() {
    // The wire hint the UI greys affordances by, and the engine gate behind
    // it. A shipped brush is rebuilt from YAML on every boot, so an edit to
    // one would appear to work and then undo itself.
    let mut engine = fresh_engine();
    engine.brush_save("my_brush", "My Brush").unwrap();

    let snap = engine.library_list();
    let shipped = snap.brushes.iter().find(|b| b.id == "ink_pen").unwrap();
    let mine = snap.brushes.iter().find(|b| b.id == "my_brush").unwrap();

    assert!(!shipped.can_edit, "a shipped brush is not the painter's");
    assert!(mine.can_edit, "one they saved is");

    // The hint is not the authority: the engine refuses regardless.
    assert!(engine.brush_rename("ink_pen", "Mine Now").is_err());
    assert!(engine.brush_delete("ink_pen").is_err());
    engine.brush_rename("my_brush", "Renamed").unwrap();
    engine.brush_delete("my_brush").unwrap();
}

#[test]
fn brush_save_rejects_an_empty_id() {
    let mut engine = fresh_engine();
    assert!(engine.brush_save("  ", "Nameless").is_err());
}

#[test]
fn brush_save_rejects_a_name_another_brush_already_has() {
    // Names are the engine's public lookup key, so two brushes sharing one
    // makes `brush_load` ambiguous. `brush_rename` has always refused this;
    // saving refuses it identically.
    let mut engine = fresh_engine();
    assert!(engine.brush_save("mine", "Ink Pen").is_err());
    assert!(engine.brush_save("mine", "  ").is_err());

    engine.brush_save("mine", "My Brush").unwrap();
    // Re-saving under the same id keeps the name: that is an update, not a
    // collision with itself.
    engine.brush_save("mine", "My Brush").unwrap();
    assert!(engine.brush_save("other", "My Brush").is_err());
}

#[test]
fn a_shipped_brush_can_be_copied_into_a_painters_pack() {
    let mut engine = fresh_engine();
    engine
        .pack_create("mine", "Mine", "", "mdi:star", palette())
        .unwrap();
    engine.pack_add_brush("mine", "ink_pen").unwrap();

    let snap = engine.library_list();
    let mine = snap.packs.iter().find(|p| p.id == "mine").unwrap();
    let basic = snap.packs.iter().find(|p| p.id == "basic").unwrap();

    assert!(mine.members.contains(&"ink_pen".to_string()));
    assert!(
        basic.members.contains(&"ink_pen".to_string()),
        "copying into a pack does not remove it from another"
    );
}

#[test]
fn mutating_a_locked_pack_is_rejected_through_the_engine() {
    let mut engine = fresh_engine();
    engine.brush_save("mine", "Mine").unwrap();

    assert!(engine.pack_add_brush("basic", "mine").is_err());
    assert!(engine.pack_remove_brush("basic", "ink_pen").is_err());
    assert!(engine.pack_delete("basic").is_err());
    assert!(engine
        .pack_edit("basic", "Renamed", "", "mdi:brush", palette())
        .is_err());

    // Nothing changed.
    let snap = engine.library_list();
    let basic = snap.packs.iter().find(|p| p.id == "basic").unwrap();
    assert_eq!(basic.name, "Basic");
    assert!(basic.members.contains(&"ink_pen".to_string()));
}

#[test]
fn a_painter_pack_is_created_edited_and_deleted() {
    let mut engine = fresh_engine();
    engine
        .pack_create("mine", "Mine", "d", "mdi:water", palette())
        .unwrap();
    engine.pack_add_brush("mine", "ink_pen").unwrap();
    engine.pack_add_brush("mine", "charcoal").unwrap();

    engine.pack_reorder_brush("mine", "charcoal", 0).unwrap();
    let snap = engine.library_list();
    let mine = snap.packs.iter().find(|p| p.id == "mine").unwrap();
    assert_eq!(mine.members, vec!["charcoal", "ink_pen"]);
    assert!(mine.can_edit_members && mine.can_edit_identity);

    let restyled = PackPalette::new("#c2521f", "#e0912b", "#e8ddc8", "#4a3826");
    engine
        .pack_edit("mine", "Renamed", "d2", "mdi:brush", restyled.clone())
        .unwrap();
    // The whole palette crosses the boundary and comes back — the roles are not
    // a thing the wire layer can quietly drop half of.
    assert_eq!(
        engine
            .library_list()
            .packs
            .iter()
            .find(|p| p.id == "mine")
            .unwrap()
            .palette,
        restyled
    );
    assert_eq!(
        engine
            .library_list()
            .packs
            .iter()
            .find(|p| p.id == "mine")
            .unwrap()
            .name,
        "Renamed"
    );

    engine.pack_delete("mine").unwrap();
    let snap = engine.library_list();
    assert!(!snap.packs.iter().any(|p| p.id == "mine"));
    // Its brushes survived, still in the packs that shipped them.
    assert!(snap.brushes.iter().any(|b| b.id == "ink_pen"));
    let basic = snap.packs.iter().find(|p| p.id == "basic").unwrap();
    assert!(basic.members.contains(&"ink_pen".to_string()));
}

#[test]
fn pack_export_import_round_trip_through_the_engine() {
    let mut engine = fresh_engine();
    engine
        .pack_create("mine", "Mine", "d", "mdi:water", palette())
        .unwrap();
    engine.brush_save("custom", "Custom").unwrap();
    engine.pack_add_brush("mine", "custom").unwrap();

    let bytes = engine.pack_export("mine").unwrap();

    // Delete both the pack and its brush, then bring them back.
    engine.pack_delete("mine").unwrap();
    engine.brush_delete("custom").unwrap();
    assert!(!engine
        .library_list()
        .brushes
        .iter()
        .any(|b| b.id == "custom"));

    let id = engine.pack_import("restored", &bytes).unwrap();
    assert_eq!(id, "restored");

    let snap = engine.library_list();
    let restored = snap.packs.iter().find(|p| p.id == "restored").unwrap();
    assert_eq!(restored.name, "Mine");
    assert_eq!(restored.icon, "mdi:water");
    assert_eq!(restored.members, vec!["custom"]);
    assert!(
        snap.brushes.iter().any(|b| b.id == "custom"),
        "the brush came back with the pack"
    );
}

#[test]
fn importing_a_pack_holding_a_brush_we_have_reuses_ours() {
    let mut engine = fresh_engine();
    engine.brush_save("my_brush", "My Brush").unwrap();
    engine
        .pack_create("mine", "Mine", "", "mdi:water", palette())
        .unwrap();
    engine.pack_add_brush("mine", "my_brush").unwrap();
    let bytes = engine.pack_export("mine").unwrap();

    let before = engine.library_list().brushes.len();
    engine.brush_rename("my_brush", "My Renamed Brush").unwrap();
    engine.pack_import("theirs", &bytes).unwrap();

    let snap = engine.library_list();
    assert_eq!(snap.brushes.len(), before, "the library did not grow");
    assert_eq!(
        snap.brushes
            .iter()
            .find(|b| b.id == "my_brush")
            .unwrap()
            .name,
        "My Renamed Brush",
        "our copy wins over the sender's"
    );
    let theirs = snap.packs.iter().find(|p| p.id == "theirs").unwrap();
    assert_eq!(theirs.name, "Mine (2)", "the colliding name is suffixed");
}

#[test]
fn importing_corrupt_bytes_is_rejected_and_changes_nothing() {
    let mut engine = fresh_engine();
    let before = engine.library_list();

    assert!(engine.pack_import("new", b"not a pack at all").is_err());

    let after = engine.library_list();
    assert_eq!(before.packs.len(), after.packs.len());
    assert_eq!(before.brushes.len(), after.brushes.len());
}

#[test]
fn renaming_a_brush_leaves_pack_membership_intact() {
    let mut engine = fresh_engine();
    engine.brush_save("my_brush", "My Brush").unwrap();
    engine
        .pack_create("mine", "Mine", "", "mdi:water", palette())
        .unwrap();
    engine.pack_add_brush("mine", "my_brush").unwrap();
    let before = engine
        .library_list()
        .packs
        .iter()
        .find(|p| p.id == "mine")
        .unwrap()
        .members
        .clone();

    engine.brush_rename("my_brush", "Fancy Nib").unwrap();

    let snap = engine.library_list();
    let mine = snap.packs.iter().find(|p| p.id == "mine").unwrap();
    assert_eq!(mine.members, before, "membership is id-keyed");
    assert_eq!(
        snap.brushes
            .iter()
            .find(|b| b.id == "my_brush")
            .unwrap()
            .name,
        "Fancy Nib"
    );
}

#[test]
fn deleting_a_brush_removes_it_from_every_pack_through_the_engine() {
    let mut engine = fresh_engine();
    engine.brush_save("my_brush", "My Brush").unwrap();
    engine
        .pack_create("mine", "Mine", "", "mdi:star", palette())
        .unwrap();
    engine.pack_add_brush("mine", "my_brush").unwrap();

    engine.brush_delete("my_brush").unwrap();

    let snap = engine.library_list();
    assert!(!snap.brushes.iter().any(|b| b.id == "my_brush"));
    for pack in &snap.packs {
        assert!(
            !pack.members.contains(&"my_brush".to_string()),
            "pack '{}' still names the deleted brush",
            pack.id
        );
    }
    assert!(engine.brush_delete("ink_pen").is_err(), "already gone");
}

#[test]
fn two_engines_share_one_library() {
    // The whole point of a process-global library: a brush saved through one
    // canvas handle is immediately visible through the next.
    let mut first = fresh_engine();
    first.brush_save("shared", "Shared").unwrap();

    let (device, queue) = test_device();
    let second = DarklyEngine::new(GpuContext::new_headless(device, queue), 64, 64);

    assert!(
        second
            .library_list()
            .brushes
            .iter()
            .any(|b| b.id == "shared"),
        "the second engine sees the first engine's brush"
    );
}

#[test]
fn brush_load_still_takes_a_name() {
    // Names stay the engine's public lookup key even though identity is an id.
    let mut engine = fresh_engine();
    engine.brush_load("Ink Pen").unwrap();
    assert!(engine.brush_load("No Such Brush").is_err());

    engine.brush_save("my_brush", "My Brush").unwrap();
    engine.brush_rename("my_brush", "Fancy Nib").unwrap();
    engine.brush_load("Fancy Nib").unwrap();
    assert!(
        engine.brush_load("My Brush").is_err(),
        "the old name no longer resolves"
    );
}
