//! An effect registered in two registries must offer exactly one add path.
//!
//! `black_and_white` and `chromatic_aberration` are each registered twice —
//! once as a veil, once as a filter — because the two registries predate their
//! unification. Both project into catalogs the add-layer picker reads, so
//! without a declared winner the picker offers one effect twice.
//!
//! The winner is declared by the effect, in its own file, via
//! `addable` — not decided by the consumer reading the catalogs.

use std::collections::{HashMap, HashSet};

/// `type_id -> addable`, for one registry.
fn veil_addability() -> HashMap<&'static str, bool> {
    darkly::gpu::veils::registrations()
        .into_iter()
        .map(|r| (r.type_id, r.addable))
        .collect()
}

fn filter_addability() -> HashMap<&'static str, bool> {
    darkly::gpu::filters::registrations()
        .into_iter()
        .map(|r| (r.type_id, r.addable))
        .collect()
}

/// Every effect registered in both registries declares exactly one add path.
///
/// This is the assertion behind "one effect appears once in the picker". It
/// fails if a future effect is added to both registries without either one
/// standing down, and it fails if a duplicate pair ever declares two winners
/// or none.
#[test]
fn duplicate_effects_declare_one_add_path() {
    let veils = veil_addability();
    let filters = filter_addability();

    let duplicates: Vec<&'static str> = veils
        .keys()
        .filter(|id| filters.contains_key(*id))
        .copied()
        .collect();

    assert!(
        !duplicates.is_empty(),
        "expected the known veil/filter duplicates to exist; if they were merged, \
         delete this test along with the `addable` field it guards"
    );

    for id in duplicates {
        let addable_count = [veils[id], filters[id]].iter().filter(|a| **a).count();
        assert_eq!(
            addable_count, 1,
            "`{id}` is registered as both a veil and a filter, so exactly one of \
             the two must declare `addable: true` — found {addable_count}. Two \
             winners puts it in the picker twice; none makes it unaddable."
        );
    }
}

/// Every effect registered in only one registry is addable.
///
/// `addable: false` exists solely to resolve a duplicate. An effect with no
/// twin that declares it would silently vanish from the add-layer modal.
#[test]
fn unique_effects_are_addable() {
    let veils = veil_addability();
    let filters = filter_addability();

    for (id, addable) in veils.iter() {
        if !filters.contains_key(id) {
            assert!(
                *addable,
                "veil `{id}` has no filter twin, so nothing owns its add path \
                 instead — `addable: false` would remove it from the picker \
                 entirely"
            );
        }
    }
    for (id, addable) in filters.iter() {
        if !veils.contains_key(id) {
            assert!(
                *addable,
                "filter `{id}` has no veil twin, so nothing owns its add path \
                 instead — `addable: false` would remove it from the picker \
                 entirely"
            );
        }
    }
}

/// The union of what the picker offers contains no repeated `type_id`.
///
/// The end-to-end form of the property, asserted over the catalogs the frontend
/// actually reads rather than over the registrations behind them.
#[test]
fn picker_offers_each_effect_once() {
    let offered: Vec<&'static str> = [darkly::gpu::veil::catalog(), darkly::gpu::filter::catalog()]
        .into_iter()
        .flat_map(|c| c.entries)
        .filter(|e| e.addable)
        .map(|e| e.type_id)
        .collect();

    let mut seen = HashSet::new();
    for id in &offered {
        assert!(
            seen.insert(*id),
            "`{id}` is offered twice by the add-layer picker; the veil and filter \
             registrations of it must not both declare `addable: true`"
        );
    }

    // The duplicated pair still reaches the frontend — suppression is an add-path
    // gate, not a removal. The Colors menu builds from the full filter catalog.
    for id in ["black_and_white", "chromatic_aberration"] {
        assert!(
            seen.contains(id),
            "`{id}` should still be offered exactly once, by whichever \
             registration owns its add path"
        );
    }
}

/// `category` is presentational: it groups picker tabs and decides nothing else.
///
/// The guard `docs/plans/effect-layers.md` §1.10 asks for. Deleting every
/// `category` declaration must change what the picker looks like and nothing
/// about what is addable, so the two facts may never be conflated.
#[test]
fn category_does_not_gate_the_add_path() {
    for entry in darkly::gpu::veil::catalog()
        .entries
        .into_iter()
        .chain(darkly::gpu::filter::catalog().entries)
    {
        assert!(
            entry.category.is_none(),
            "`{}` declares a category; categories land with the registry merge. \
             When they do, this assertion becomes: addability must not correlate \
             with category — `addable` is the only add-path gate.",
            entry.type_id
        );
    }
}
