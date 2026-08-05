//! The rendered documentation assets: that every previewable entry declares
//! motion, that the motion is real, and that what lands on disk is what the
//! catalogs said it would be.
//!
//! Two groups live here. The first is GPU-free — it reads
//! [`CATALOG_RENDERERS`] and the four registries and validates the *declarations*
//! before any device is touched. That group catches the whole class of defect a
//! pixel test structurally cannot: a recipe whose keyframes step rather than
//! blend renders a slideshow whose parameters genuinely are constant across each
//! held stretch, so every "the frames differ when the parameters differ"
//! assertion is vacuously satisfied. The defect is in the declaration, and this
//! is where it is caught.
//!
//! The second group renders. Those tests share one fixture that runs the whole
//! walk once for the test binary rather than once per test, and tests that only
//! need a single entry call `render_entry` directly and skip the PNG round-trip
//! entirely.
//!
//! Run with: `cargo test -p darkly --features testing --test docs_render -- --test-threads=1`

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use darkly::catalog::catalogs;
use darkly::docs_render::{self, frame_t, Gpu, Manifest, Rendered, CATALOG_RENDERERS};
use darkly::gpu::params::{ParamDef, ParamValue};
use darkly::gpu::preview_recipe::{PreviewSpec, TrackTarget};
use darkly::gpu::veil::CATALOG_ID as VEILS;

// ---------------------------------------------------------------------------
// Shared enumeration — one source for every test below
// ---------------------------------------------------------------------------

/// One previewable entry: where it lives, how it moves, what knobs it may name,
/// and its own parameter schema.
struct Previewable {
    catalog: &'static str,
    type_id: &'static str,
    spec: PreviewSpec,
    defs: &'static [ParamDef],
}

/// Every entry the walk will reach, resolved through the same table the walk and
/// the renderers use. Nothing here carries a list of catalogs or of entries.
fn previewable() -> Vec<Previewable> {
    let mut out = Vec::new();
    for cat in catalogs() {
        let Some(cr) = CATALOG_RENDERERS.iter().find(|c| c.id == cat.id) else {
            assert!(
                cat.entries.iter().all(|e| !e.supports_preview),
                "catalog `{}` has previewable entries and no renderer",
                cat.id
            );
            continue;
        };
        for e in cat.entries.iter().filter(|e| e.supports_preview) {
            let spec = (cr.spec)(e.type_id).unwrap_or_else(|| {
                panic!(
                    "`{}/{}` is previewable but hands out no recipe",
                    cat.id, e.type_id
                )
            });
            out.push(Previewable {
                catalog: cr.id,
                type_id: e.type_id,
                spec,
                defs: (cr.defs)(e.type_id),
            });
        }
    }
    assert!(!out.is_empty(), "no previewable entries found at all");
    out
}

// ---------------------------------------------------------------------------
// The declarations — GPU-free
// ---------------------------------------------------------------------------

/// Every filter, every veil, every blend mode and `noise` hands out a recipe.
///
/// Driven off the four **registries** rather than a hand-written list, so
/// adding a filter without a recipe fails here — which is the whole point of
/// putting the recipe on the registration.
#[test]
fn every_previewable_entry_declares_a_recipe() {
    let filters = darkly::gpu::filter::FilterPipelineRegistry::new();
    for reg in filters.types() {
        assert!(
            filters.preview(reg.type_id).is_some(),
            "filter `{}` declares no preview recipe",
            reg.type_id
        );
    }
    let veils = darkly::gpu::veil::VeilRegistry::new();
    for reg in veils.types() {
        assert!(
            veils.preview(reg.type_id).is_some(),
            "veil `{}` declares no preview recipe",
            reg.type_id
        );
    }
    for reg in darkly::gpu::blend_mode::registry().all() {
        assert!(
            darkly::gpu::blend_mode::registry()
                .preview(reg.type_id)
                .is_some(),
            "blend mode `{}` declares no preview recipe",
            reg.type_id
        );
    }
    assert!(darkly::gpu::void::VoidRegistry::new()
        .preview("noise")
        .is_some());
}

/// Two schemas are shared by two registrations each, and so are their recipes —
/// at the same address, not merely equal.
///
/// An `==` check would pass two copies that have not drifted yet; pointer
/// identity fails the moment someone writes the recipe out twice, which is the
/// duplication the shared `static` exists to make impossible.
#[test]
fn shared_schemas_share_one_recipe() {
    let filters = darkly::gpu::filter::FilterPipelineRegistry::new();
    let veils = darkly::gpu::veil::VeilRegistry::new();
    for shared in ["black_and_white", "chromatic_aberration"] {
        let f = filters.preview(shared).unwrap();
        let v = veils.preview(shared).unwrap();
        assert!(
            std::ptr::eq(f.recipe, v.recipe),
            "`{shared}`'s filter and veil hold two different recipes"
        );
    }
}

/// A void's previewability *is* its recipe — one fact, not two that can drift.
#[test]
fn void_previewability_is_the_recipe() {
    let voids = darkly::gpu::void::VoidRegistry::new();
    let entries = catalogs()
        .into_iter()
        .find(|c| c.id == darkly::gpu::void::CATALOG_ID)
        .expect("the voids catalog")
        .entries;
    assert_eq!(entries.len(), 4);
    for e in &entries {
        assert_eq!(
            e.supports_preview,
            voids.preview(e.type_id).is_some(),
            "`voids/{}` answers previewability two different ways",
            e.type_id
        );
        assert_eq!(
            e.supports_preview,
            e.type_id == "noise",
            "`voids/{}` previewability",
            e.type_id
        );
    }
}

/// Every recipe declares at least one track, and at least one of them holds two
/// keyframes with different values. A recipe with nothing moving would render
/// its whole frame budget as one image — caught before a GPU is touched.
#[test]
fn every_recipe_moves() {
    for p in previewable() {
        let tracks = p.spec.recipe.tracks;
        assert!(
            !tracks.is_empty(),
            "`{}/{}` declares no tracks",
            p.catalog,
            p.type_id
        );
        assert!(
            tracks
                .iter()
                .any(|tr| tr.keys.windows(2).any(|w| w[0].value != w[1].value)),
            "`{}/{}` has tracks but none of them moves",
            p.catalog,
            p.type_id
        );
    }
}

/// Every track's keys ascend in `t`, sit inside `0.0 ..= 1.0`, and start at
/// zero — the evaluator's precondition, stated where it can be checked.
#[test]
fn keyframes_are_ordered_and_start_at_zero() {
    for p in previewable() {
        for tr in p.spec.recipe.tracks {
            let where_ = format!("`{}/{}`", p.catalog, p.type_id);
            assert!(!tr.keys.is_empty(), "{where_} declares an empty track");
            assert_eq!(tr.keys[0].t, 0.0, "{where_} does not start at t = 0");
            for w in tr.keys.windows(2) {
                assert!(w[0].t < w[1].t, "{where_} has keys out of order");
            }
            assert!(
                tr.keys.last().unwrap().t <= 1.0,
                "{where_} runs past the end of the timeline"
            );
        }
    }
}

/// A keyframe pair whose values differ, on a parameter kind that interpolates,
/// must blend to a value equal to **neither** endpoint.
///
/// This is what catches a recipe that steps where its author believed it
/// blended — a curve whose keyframes disagree on control-point count, say,
/// which renders four stills held for twelve frames each. No pixel-level
/// assertion can see that: a stepping recipe's parameters genuinely *are*
/// constant across each held stretch, so "the frames differ when the parameters
/// differ" is vacuously satisfied. The defect is in the declaration.
#[test]
fn every_interpolating_track_actually_interpolates() {
    for p in previewable() {
        for tr in p.spec.recipe.tracks {
            let name = match tr.target {
                TrackTarget::Param(n) => n,
                TrackTarget::Layer(n) => n,
                // A time track is a bare `f32` the evaluator blends directly;
                // it has no schema and no kind that could decline to blend.
                TrackTarget::Time => continue,
            };
            let namespace = match tr.target {
                TrackTarget::Layer(_) => p.spec.layer_knobs,
                _ => p.defs,
            };
            let def = namespace
                .iter()
                .find(|d| d.name == name)
                .unwrap_or_else(|| panic!("`{}/{}` drives unknown `{name}`", p.catalog, p.type_id));
            if !def.interpolates() {
                continue;
            }
            for w in tr.keys.windows(2) {
                let (a, b) = (
                    def.value_from_const(w[0].value),
                    def.value_from_const(w[1].value),
                );
                if a == b {
                    continue;
                }
                // Two adjacent integers have nothing between them, so rounding
                // legitimately lands on an endpoint. Every authored integer
                // track spans further than that.
                if let (ParamValue::Int(x), ParamValue::Int(y)) = (&a, &b) {
                    if (x - y).abs() <= 1 {
                        continue;
                    }
                }
                let mid = def.lerp(&a, &b, 0.5);
                assert!(
                    mid != a && mid != b,
                    "`{}/{}`'s `{name}` steps between two differing keyframes — \
                     the asset would be a slideshow, not an animation",
                    p.catalog,
                    p.type_id
                );
            }
        }
    }
}

/// A veil recipe may not combine a time track with parameter tracks.
///
/// A veil has no in-place parameter update — the shipped path rebuilds — and a
/// rebuild resets the instance's clock, so a fresh instance cannot be
/// fast-forwarded to an arbitrary elapsed time. The day a veil wants both, this
/// is the red test that forces the `Veil::update_params` conversation rather
/// than letting a silently-restarting clock ship.
#[test]
fn no_veil_recipe_mixes_time_and_param_tracks() {
    for p in previewable().into_iter().filter(|p| p.catalog == VEILS) {
        let tracks = p.spec.recipe.tracks;
        let time = tracks
            .iter()
            .any(|tr| matches!(tr.target, TrackTarget::Time));
        let params = tracks
            .iter()
            .any(|tr| matches!(tr.target, TrackTarget::Param(_)));
        assert!(
            !(time && params),
            "veil `{}` drives both its clock and its parameters; rebuilding it \
             for a parameter change would reset that clock",
            p.type_id
        );
    }
}

/// Every track resolves against the namespace its own catalog hands it.
///
/// The test carries **no list of catalogs and no list of knobs** — it asks the
/// same [`PreviewSpec`] the renderer uses. A veil that named a layer knob fails
/// here against its own catalog's empty knob set, and a typo'd parameter fails
/// on the parameter half.
#[test]
fn every_track_target_resolves() {
    for p in previewable() {
        for t in [0.0, 0.5, 1.0] {
            assert!(
                p.spec.recipe.layer_at(p.spec.layer_knobs, t).is_ok(),
                "`{}/{}` drives a layer knob its catalog does not expose",
                p.catalog,
                p.type_id
            );
            assert_eq!(
                p.spec.recipe.params_at(p.defs, t).len(),
                p.defs.len(),
                "`{}/{}` does not evaluate to its own schema's shape",
                p.catalog,
                p.type_id
            );
        }
        for tr in p.spec.recipe.tracks {
            if let TrackTarget::Param(n) = tr.target {
                assert!(
                    p.defs.iter().any(|d| d.name == n),
                    "`{}/{}` drives `{n}`, which is not in its schema",
                    p.catalog,
                    p.type_id
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The rendered assets
// ---------------------------------------------------------------------------

/// Where the shared fixture writes. A **fixed** name, not one keyed by pid, so a
/// run that panicked or was killed reclaims its predecessor's space instead of
/// adding to it. Safe because `--test-threads=1` is mandatory for GPU tests and
/// this is the only test binary that writes here.
fn assets_dir() -> PathBuf {
    std::env::temp_dir().join("darkly-docs-render-assets")
}

/// The whole walk, run once for the test binary.
fn assets() -> &'static (PathBuf, Manifest) {
    static ASSETS: OnceLock<(PathBuf, Manifest)> = OnceLock::new();
    ASSETS.get_or_init(|| {
        let dir = assets_dir();
        let _ = std::fs::remove_dir_all(&dir);
        let manifest = docs_render::render_all(&dir).expect("render_all");
        (dir, manifest)
    })
}

/// One entry rendered on its own, without the PNG round-trip.
///
/// Darkly's GPU state is deliberately not `Send` — the engine is single-threaded
/// everywhere — so the device is an ordinary local rather than a shared static.
/// Each test that renders holds one for the whole of its own work, which is also
/// what lets the tests that care about cross-asset leakage drive several entries
/// through the same documents.
fn render_one(gpu: &mut Gpu, catalog: &str, type_id: &str) -> Rendered {
    docs_render::render_entry(gpu, catalog, type_id).expect("render_entry")
}

/// Every `(catalog, entry)` directory actually present under `root`.
fn dirs_on_disk(root: &Path) -> BTreeSet<(String, String)> {
    let mut out = BTreeSet::new();
    for cat in std::fs::read_dir(root).expect("the output directory") {
        let cat = cat.unwrap().path();
        if !cat.is_dir() {
            continue;
        }
        let cat_name = cat.file_name().unwrap().to_string_lossy().to_string();
        for entry in std::fs::read_dir(&cat).unwrap() {
            let entry = entry.unwrap().path();
            if entry.is_dir() {
                out.insert((
                    cat_name.clone(),
                    entry.file_name().unwrap().to_string_lossy().to_string(),
                ));
            }
        }
    }
    out
}

/// The full walk succeeds — no previewable entry was missed by the renderer
/// table. This is what fails the day a new previewable registry is added.
#[test]
fn every_previewable_entry_has_a_renderer() {
    let (_, manifest) = assets();
    assert!(!manifest.assets.is_empty());
}

/// Seven filters, ten veils, one void and sixteen blend modes — counted **per
/// catalog**. A bare total of thirty-four would not notice a whole catalog
/// dropping out and another gaining entries.
#[test]
fn all_thirty_four_assets_land() {
    let (_, manifest) = assets();
    let counts: BTreeMap<&str, usize> = manifest
        .assets
        .iter()
        .map(|(k, v)| (k.as_str(), v.len()))
        .collect();
    assert_eq!(
        counts,
        BTreeMap::from([
            ("filters", 7),
            ("veils", 10),
            ("voids", 1),
            ("blendModes", 16),
        ])
    );
    assert_eq!(counts.values().sum::<usize>(), 34);
}

/// The set of directories **found by walking the output** equals the previewable
/// set the catalogs declare, in both directions — and the index agrees with the
/// directory.
///
/// Scanning the tree rather than trusting the manifest is the difference between
/// testing the artifact and testing the binary's own claim about it.
///
/// Its honest limit: the expectation and the walk share `catalogs()`, so this
/// catches an entry the walker skipped and an asset nothing declares, but not
/// `catalogs()` itself under-reporting a registry. `every_previewable_entry_declares_a_recipe`
/// closes the half this file can, by reading the registries directly.
#[test]
fn assets_on_disk_match_the_previewable_set() {
    let (dir, manifest) = assets();

    let declared: BTreeSet<(String, String)> = catalogs()
        .iter()
        .flat_map(|c| {
            c.entries
                .iter()
                .filter(|e| e.supports_preview)
                .map(|e| (c.id.to_string(), e.type_id.to_string()))
        })
        .collect();
    assert_eq!(dirs_on_disk(dir), declared);

    let indexed: BTreeSet<(String, String)> = manifest
        .assets
        .iter()
        .flat_map(|(c, es)| es.keys().map(move |t| (c.clone(), t.clone())))
        .collect();
    assert_eq!(indexed, declared);
}

/// Every written PNG decodes to the declared size, and the index agrees.
///
/// Pins the coincidence the layout depends on: the offscreen veil and void
/// renderers are hard-wired to fit into the picker's preview box, and the
/// document path is sized to match it. If that constant ever moved, veil and
/// void frames would silently diverge in size from the rest.
#[test]
fn every_frame_is_the_same_size() {
    let (dir, manifest) = assets();
    let dim = darkly::docs_render::subject::DOCS_SUBJECT_DIM;
    assert_eq!((manifest.width, manifest.height), (dim, dim));

    let mut checked = 0usize;
    for entries in manifest.assets.values() {
        for asset in entries.values() {
            let path = dir.join(&asset.dir).join("000.png");
            let img = image::open(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            assert_eq!((img.width(), img.height()), (dim, dim), "{}", asset.dir);
            checked += 1;
        }
    }
    assert_eq!(checked, 34);
}

/// For every asset the PNG count equals the recipe's frame count, and the
/// index's `frames` / `fps` / `loop` are the recipe's own — which is what makes
/// `loop` in the artifact something a consumer can rely on rather than a claim.
#[test]
fn manifest_frames_fps_and_loop_match_the_recipe() {
    let (dir, manifest) = assets();
    let mut non_looping = BTreeSet::new();

    for p in previewable() {
        let asset = &manifest.assets[p.catalog][p.type_id];
        let recipe = p.spec.recipe;
        assert_eq!(asset.frames, recipe.frames, "{}/{}", p.catalog, p.type_id);
        assert_eq!(asset.fps, recipe.fps, "{}/{}", p.catalog, p.type_id);
        assert_eq!(asset.loops, recipe.loops(), "{}/{}", p.catalog, p.type_id);

        let written = std::fs::read_dir(dir.join(&asset.dir)).unwrap().count();
        assert_eq!(
            written as u32, recipe.frames,
            "`{}/{}` wrote {written} frames",
            p.catalog, p.type_id
        );
        if !asset.loops {
            non_looping.insert(format!("{}/{}", p.catalog, p.type_id));
        }
    }

    // The three time-driven veils integrate their clocks forward and are
    // recorded honestly rather than being made to loop by a shader change.
    assert_eq!(
        non_looping,
        BTreeSet::from([
            "veils/grain".to_string(),
            "veils/rainy_glass".to_string(),
            "veils/vhs".to_string(),
        ])
    );

    // Alphabetically the last of the six tests that read the fixture, and under
    // the mandatory single test thread that makes it the last to run — so the
    // 65–160 MB of frames come back here rather than being left on every
    // developer machine and every CI run. The fixture also clears any earlier
    // tree before writing, so a run that panicked or was killed reclaims its
    // predecessor's space rather than adding to it.
    std::fs::remove_dir_all(dir).expect("the fixture cleans up after itself");
}

/// Every one of the thirty-four renders at least two distinct images.
///
/// The floor rather than the ceiling: proving the motion is *continuous* is
/// `every_interpolating_track_actually_interpolates`'s job, GPU-free and with a
/// far better error message.
#[test]
fn every_asset_has_real_motion() {
    let (dir, manifest) = assets();
    for entries in manifest.assets.values() {
        for (type_id, asset) in entries {
            let first = std::fs::read(dir.join(&asset.dir).join("000.png")).unwrap();
            let moved = (1..asset.frames).any(|i| {
                std::fs::read(dir.join(&asset.dir).join(format!("{i:03}.png")))
                    .map(|f| f != first)
                    .unwrap_or(false)
            });
            assert!(
                moved,
                "`{}` rendered {} identical frames",
                type_id, asset.frames
            );
        }
    }
}

/// Whether two evaluated parameter values are the same *to the renderer*.
///
/// A ping-pong recipe reaches the same value at `t` and at `1 − t` by two
/// different arithmetic paths, so their floats agree to about a part in ten
/// million rather than bit-for-bit. The tolerance here is four orders of
/// magnitude coarser than that noise and still an order of magnitude finer than
/// one code value of an 8-bit channel, so it merges exactly the pairs that are
/// mathematically the same state and separates every pair that could possibly
/// render differently.
fn approx_eq(a: &ParamValue, b: &ParamValue) -> bool {
    const EPS: f32 = 1e-4;
    let close = |x: f32, y: f32| (x - y).abs() <= EPS;
    let all =
        |x: &[f32], y: &[f32]| x.len() == y.len() && x.iter().zip(y).all(|(a, b)| close(*a, *b));
    match (a, b) {
        (ParamValue::Float(x), ParamValue::Float(y)) => close(*x, *y),
        (ParamValue::Color(x), ParamValue::Color(y)) => all(x, y),
        (ParamValue::Vec2(x), ParamValue::Vec2(y)) => all(x, y),
        (ParamValue::Levels(x), ParamValue::Levels(y)) => all(x, y),
        (ParamValue::Curve(x), ParamValue::Curve(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| all(p, q))
        }
        (ParamValue::List(x), ParamValue::List(y)) => {
            x.len() == y.len()
                && x.iter().zip(y).all(|(ea, eb)| {
                    ea.len() == eb.len()
                        && ea
                            .iter()
                            .zip(eb)
                            .all(|((ka, va), (kb, vb))| ka == kb && approx_eq(va, vb))
                })
        }
        _ => a == b,
    }
}

fn states_equal(a: &[ParamValue], b: &[ParamValue]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| approx_eq(x, y))
}

/// Two frames render the same image **exactly when** their evaluated states are
/// the same.
///
/// Forward — different states must render differently — is the strong form of
/// "consecutive frames differ": a recipe whose per-frame delta is too small to
/// move a single code value fails here, which is the whole class of defect that
/// makes an asset look frozen.
///
/// Backward — the same state must render the same image — is what guards
/// determinism, the veil rebuild path, and the two shared documents: a mode or a
/// filter that left state behind would show up as a large divergence between two
/// frames that ought to match. Recipes that ping-pong revisit the same state at
/// `t` and `1 − t`, so this half has real content rather than being vacuous.
///
/// The two halves are asymmetric on purpose. "Different" is exact — one byte
/// anywhere is enough. "The same" allows one code value, because a ping-pong
/// reaches its shared state by two arithmetic paths whose floats agree to about
/// a part in ten million, and a parameter that lands either side of a rounding
/// boundary moves a handful of pixels by exactly one. Byte-for-byte determinism
/// is pinned where it is meaningful — on two renders of the *same* `t` — by
/// [`a_looping_recipe_renders_a_seamless_handoff`].
#[test]
fn distinct_frames_match_distinct_parameter_states() {
    let mut gpu = Gpu::new();
    let mut revisited = 0usize;
    for p in previewable() {
        let rendered = render_one(&mut gpu, p.catalog, p.type_id);
        let recipe = p.spec.recipe;
        let state = |i: u32| {
            let t = frame_t(i, recipe.frames);
            let mut vals = recipe.params_at(p.defs, t);
            vals.extend(
                recipe
                    .layer_at(p.spec.layer_knobs, t)
                    .unwrap()
                    .into_iter()
                    .map(|(_, v)| v),
            );
            vals.push(ParamValue::Float(recipe.time_at(t)));
            vals
        };

        let mut equal_states = 0usize;
        for i in 0..recipe.frames {
            for j in (i + 1)..recipe.frames {
                let (a, b) = (&rendered.frames[i as usize], &rendered.frames[j as usize]);
                let where_ = format!("`{}/{}` frames {i} and {j}", p.catalog, p.type_id);
                if states_equal(&state(i), &state(j)) {
                    equal_states += 1;
                    let worst = a
                        .iter()
                        .zip(b)
                        .map(|(x, y)| x.abs_diff(*y))
                        .max()
                        .unwrap_or(0);
                    assert!(
                        worst <= 1,
                        "{where_} hold the same state but differ by {worst} code values"
                    );
                } else {
                    assert_ne!(
                        a, b,
                        "{where_} hold different states and render identically"
                    );
                }
            }
        }
        revisited += equal_states;
    }

    // The backward half is not vacuous: across the set, recipes really do
    // revisit their own states. (Not every one does — `hsv` closes its loop only
    // at `t = 1.0`, the frame after the last, so its forty-eight sampled states
    // are all distinct even though it loops.)
    assert!(
        revisited > 0,
        "no asset revisited a state, so the equal-state half proved nothing"
    );
}

/// An extra frame rendered at `t = 1.0` — the frame *after* the last — is
/// byte-identical to frame 0 for a looping recipe.
///
/// Described honestly: since `loops()` means `params_at(1.0) == params_at(0.0)`
/// by definition, what this proves is **render determinism at the wrap point** —
/// that the same evaluated state renders the same pixels through a full document
/// round-trip. That is worth having; it is not a proof that the loop rule is
/// well-chosen.
#[test]
fn a_looping_recipe_renders_a_seamless_handoff() {
    let spec = darkly::gpu::filter::FilterPipelineRegistry::new()
        .preview("hsv")
        .unwrap();
    assert!(spec.recipe.loops());
    let defs = darkly::gpu::filter::FilterPipelineRegistry::new().params("hsv");
    assert_eq!(
        spec.recipe.params_at(defs, 1.0),
        spec.recipe.params_at(defs, 0.0)
    );

    let mut gpu = Gpu::new();
    let rendered = render_one(&mut gpu, "filters", "hsv");
    let again = render_one(&mut gpu, "filters", "hsv");
    assert_eq!(
        rendered.frames[0], again.frames[0],
        "the same state rendered two different images"
    );
}

/// Value-pinned, not merely "it differs": at the frame where the opacity track
/// reads 1.0, the composite is exactly `255 - c` per RGB channel of the subject.
///
/// Pins three things at once — the filter ran, an isolated *group's* opacity
/// really is applied to its accumulator (no existing test covers that; the five
/// `set_opacity` assertions in this suite are all on raster layers), and no
/// state leaked from the six filters rendered before it through the same
/// document.
#[test]
fn invert_frame_at_full_opacity_is_the_exact_inverse_of_the_subject() {
    let dim = darkly::docs_render::subject::DOCS_SUBJECT_DIM;
    let subject = darkly::docs_render::subject::subject_rgba(dim);

    // Rendered *after* every other filter through the same document, so this
    // also pins that none of them left state behind.
    let mut gpu = Gpu::new();
    let mut rendered = None;
    for reg in darkly::gpu::filter::FilterPipelineRegistry::new().types() {
        let r = render_one(&mut gpu, "filters", reg.type_id);
        if reg.type_id == "invert" {
            rendered = Some(r);
        }
    }
    let rendered = rendered.expect("invert is registered");

    let spec = darkly::gpu::filter::FilterPipelineRegistry::new()
        .preview("invert")
        .unwrap();
    let full = (0..spec.recipe.frames)
        .find(|i| {
            spec.recipe
                .layer_at(spec.layer_knobs, frame_t(*i, spec.recipe.frames))
                .unwrap()
                == vec![("opacity", ParamValue::Float(1.0))]
        })
        .expect("the opacity track reaches 1.0");

    let frame = &rendered.frames[full as usize];
    assert_eq!(frame.len(), subject.len());
    for (i, (out, src)) in frame
        .chunks_exact(4)
        .zip(subject.chunks_exact(4))
        .enumerate()
    {
        assert_eq!(
            [out[0], out[1], out[2], out[3]],
            [255 - src[0], 255 - src[1], 255 - src[2], src[3]],
            "pixel {i} is not the exact inverse of the subject"
        );
    }
}

/// At the frame where the tint reads zero, every pixel of the black-and-white
/// veil is neutral grey — so the offscreen veil path applied the veil rather
/// than writing the subject through.
///
/// Deliberately a *relational* pin rather than an absolute grey value: the veil
/// path's subject is the area-averaged 2× field, so pinning a fixed number would
/// be pinning the resample rather than the veil.
#[test]
fn black_and_white_veil_frame_is_neutral_gray() {
    let spec = darkly::gpu::veil::VeilRegistry::new()
        .preview("black_and_white")
        .unwrap();
    let defs = darkly::gpu::veil::VeilRegistry::new().param_defs("black_and_white");
    let tint = defs
        .iter()
        .position(|d| d.name == "tint_strength")
        .expect("the shared schema declares a tint strength");
    let untinted = (0..spec.recipe.frames)
        .find(|i| {
            spec.recipe.params_at(defs, frame_t(*i, spec.recipe.frames))[tint]
                == ParamValue::Float(0.0)
        })
        .expect("the tint track reaches zero");

    let rendered = render_one(&mut Gpu::new(), "veils", "black_and_white");
    let frame = &rendered.frames[untinted as usize];
    for (i, px) in frame.chunks_exact(4).enumerate() {
        assert!(
            px[0] == px[1] && px[1] == px[2],
            "pixel {i} is {:?}, not neutral grey",
            &px[..3]
        );
    }
}

/// Every noise frame holds more than one distinct value. A void that failed to
/// render — or whose aux texture was still a placeholder — produces a flat
/// image, which is exactly the failure the stream voids opt out of preview to
/// avoid, checked here on the one void that opts in.
#[test]
fn noise_void_frames_are_not_uniform() {
    let rendered = render_one(&mut Gpu::new(), "voids", "noise");
    for (i, frame) in rendered.frames.iter().enumerate() {
        let first: &[u8] = &frame[..4];
        assert!(
            frame.chunks_exact(4).any(|px| px != first),
            "noise frame {i} is a flat colour"
        );
    }
}

/// At the frame where the shared opacity track reads 1.0, no two of the sixteen
/// modes render the same image.
///
/// Sixteen identical assets is the failure this whole path exists to avoid. It
/// doubles as the guard on the blend source's colour choice and on the shared
/// blend-mode document — a mode that leaked its predecessor's shader value would
/// show up here as a duplicate. Deliberately *not* asserted at frame 0, where
/// the top layer is invisible and all sixteen are correctly identical.
#[test]
fn blend_mode_frames_at_full_opacity_are_pairwise_distinct() {
    let modes: Vec<&str> = darkly::gpu::blend_mode::registry()
        .all()
        .into_iter()
        .map(|r| r.type_id)
        .collect();
    assert_eq!(modes.len(), 16);

    let spec = darkly::gpu::blend_mode::registry()
        .preview("normal")
        .unwrap();
    let full = (0..spec.recipe.frames)
        .find(|i| {
            spec.recipe
                .layer_at(spec.layer_knobs, frame_t(*i, spec.recipe.frames))
                .unwrap()
                == vec![("opacity", ParamValue::Float(1.0))]
        })
        .expect("the shared opacity track reaches 1.0");

    let mut gpu = Gpu::new();
    let mut seen: Vec<(&str, Vec<u8>)> = Vec::new();
    for mode in modes {
        let frame = render_one(&mut gpu, "blendModes", mode).frames[full as usize].clone();
        if let Some((other, _)) = seen.iter().find(|(_, f)| *f == frame) {
            panic!("blend modes `{other}` and `{mode}` render the same image");
        }
        seen.push((mode, frame));
    }

    // The same frame index is correctly identical across modes at zero opacity,
    // which is why the assertion above is taken at full opacity instead.
    let a = render_one(&mut gpu, "blendModes", "multiply").frames[0].clone();
    let b = render_one(&mut gpu, "blendModes", "screen").frames[0].clone();
    assert_eq!(a, b, "at zero opacity every mode is the bare backdrop");
}

// ---------------------------------------------------------------------------
// Arguments
// ---------------------------------------------------------------------------

/// The binary's only logic. It lives in the library because coverage tooling
/// runs test targets and never executes a `[[bin]]` — a `parse_args` left inside
/// `fn main` would be permanently uncovered.
#[test]
fn parse_args_reads_out() {
    let args = docs_render::parse_args(["--out".to_string(), "/tmp/x".to_string()].into_iter())
        .expect("--out <dir> parses");
    assert_eq!(args.out, Some(PathBuf::from("/tmp/x")));

    // `--help` is not an error, and it names no work to do.
    let args = docs_render::parse_args(["--help".to_string()].into_iter()).unwrap();
    assert_eq!(args.out, None);
}

#[test]
fn parse_args_rejects_a_missing_out() {
    assert!(docs_render::parse_args(std::iter::empty()).is_err());
    assert!(docs_render::parse_args(["--out".to_string()].into_iter()).is_err());
    assert!(docs_render::parse_args(["--wat".to_string()].into_iter()).is_err());
}
