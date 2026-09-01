//! `category` is what the user reads; it is not what the engine acts on.
//!
//! One registry replaced two, so the split a user sees between Filters and
//! Veils is now a declaration rather than a structure. These tests pin both
//! halves of that: that the declaration partitions the catalog exactly, and
//! that nothing behavioural is allowed to read it.
//!
//! The second is the load-bearing one. A category that quietly gated
//! capability would rebuild the two subsystems this work removed, one field at
//! a time, and it would do so invisibly — which is the failure mode CLAUDE.md's
//! type-owned-dispatch rule names.

use std::collections::{BTreeMap, BTreeSet};

use darkly::gpu::effect::{catalog, EffectRegistry};

/// The two categories the UI shows, and nothing else.
const CATEGORIES: [&str; 2] = ["Filters", "Veils"];

/// Every effect declares exactly one category, and it is one of the two the UI
/// renders a tab for.
///
/// A misspelling or a third value would silently produce a tab nobody meant to
/// add, since the rail is derived from whatever the entries declare.
#[test]
fn every_effect_declares_a_known_category() {
    let registry = EffectRegistry::new();
    let regs = registry.registrations();
    assert!(!regs.is_empty(), "the effect registry is empty");

    for reg in regs {
        assert!(
            CATEGORIES.contains(&reg.category),
            "`{}` declares category `{}`, which is not one of {CATEGORIES:?}",
            reg.type_id,
            reg.category
        );
    }
}

/// The declared split, entry by entry.
///
/// The dividing rule: a Filter is a function of one texel's colour; a Veil
/// reads its neighbours, its clock, or both. Spelled out rather than derived so
/// that moving an effect across the line is a deliberate edit to this list
/// rather than something that happens on its own.
#[test]
fn the_categories_partition_the_catalog() {
    let registry = EffectRegistry::new();
    let mut by_category: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for reg in registry.registrations() {
        by_category
            .entry(reg.category)
            .or_default()
            .insert(reg.type_id);
    }

    assert_eq!(
        by_category,
        BTreeMap::from([
            (
                "Filters",
                BTreeSet::from([
                    "black_and_white",
                    "brightness_contrast",
                    "curves",
                    "hsv",
                    "invert",
                    "levels",
                ])
            ),
            (
                "Veils",
                BTreeSet::from([
                    "chromatic_aberration",
                    "frozen",
                    "grain",
                    "lens_blur",
                    "painting",
                    "pixelate",
                    "rainy_glass",
                    "vhs",
                ])
            ),
        ])
    );
}

/// Category decides nothing about capability.
///
/// Every effect is constructible, previewable and destructively applicable
/// regardless of which tab it appears under — the property that makes the split
/// presentational. If a future change gates any of these on category, this
/// fails before the two subsystems can grow back.
#[test]
fn category_gates_no_capability() {
    let registry = EffectRegistry::new();
    for reg in registry.registrations() {
        assert!(
            !reg.hotkey_action.is_empty(),
            "`{}` declares no action, so it cannot be applied destructively — \
             every effect can be, whatever its category",
            reg.type_id
        );
        assert!(
            !reg.icon.is_empty(),
            "`{}` declares no icon; every effect needs one for its tree row and \
             menu entry, whatever its category",
            reg.type_id
        );
        assert!(
            reg.preview.is_some(),
            "`{}` declares no preview; every effect is previewable, whatever \
             its category",
            reg.type_id
        );
        assert!(
            reg.targets.contains(&wgpu::TextureFormat::Rgba8Unorm),
            "`{}` cannot render into a colour target",
            reg.type_id
        );
    }
}

/// One catalog, sorted so a consumer can run-length group it into its category
/// headings without bucketing — the same shape the blend-mode dropdown reads.
#[test]
fn the_catalog_is_sorted_by_category_then_name() {
    let entries = catalog().entries;
    let keys: Vec<(Option<&str>, &str)> = entries
        .iter()
        .map(|e| (e.category, e.display_name))
        .collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(
        keys, sorted,
        "the catalog must be emitted in (category, display_name) order"
    );

    // Run-length grouping is only correct if each category is contiguous.
    let mut seen = BTreeSet::new();
    let mut previous: Option<Option<&str>> = None;
    for (category, _) in &keys {
        if previous != Some(*category) {
            assert!(
                seen.insert(*category),
                "category {category:?} appears in two separate runs"
            );
            previous = Some(*category);
        }
    }
}

/// The duplicate entries the whole merge exists to remove are gone.
///
/// `black_and_white` and `chromatic_aberration` were each registered twice —
/// once as a veil, once as a filter — and appeared twice in the picker. One
/// catalog now holds one of each. This is the reported bug, asserted where a
/// user would see it.
#[test]
fn no_effect_appears_twice() {
    let entries = catalog().entries;
    let mut seen = BTreeSet::new();
    for entry in &entries {
        assert!(
            seen.insert(entry.type_id),
            "`{}` appears twice in the effect catalog",
            entry.type_id
        );
    }
    for id in ["black_and_white", "chromatic_aberration"] {
        assert!(
            seen.contains(id),
            "`{id}` should still be offered, exactly once"
        );
    }
}
