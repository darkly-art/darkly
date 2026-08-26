//! The rendered documentation assets: that every previewable entry declares a
//! preview, that what it renders moves, and that what lands on disk is what the
//! catalogs said it would be.
//!
//! Two groups live here. The first is GPU-free and reads the registries: what
//! an entry *declares* — that it has a preview, how long it runs, whether it
//! closes — is data, and can be checked before a device is touched.
//!
//! The second group renders. Motion itself is a method rather than a
//! declaration, so there is nothing left to inspect statically and the pixels
//! are the only witness; `tests/picker_preview.rs` holds the finer-grained half
//! of that, over the same driver. Those tests share one fixture that runs the
//! whole walk once for the test binary rather than once per test, and tests that
//! only need a single entry call `render_entry` directly and skip the PNG
//! round-trip entirely.
//!
//! Run with: `cargo test -p darkly --features testing --test docs_render -- --test-threads=1`

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use darkly::catalog::{catalogs, preview_mechanisms};
use darkly::docs_render::{self, Gpu, Manifest, Rendered};
use darkly::gpu::params::ParamValue;
use darkly::gpu::preview::{frame_t, PreviewAnim};

// ---------------------------------------------------------------------------
// Shared enumeration — one source for every test below
// ---------------------------------------------------------------------------

/// One previewable entry: where it lives and how long its preview runs.
struct Previewable {
    catalog: &'static str,
    type_id: &'static str,
    anim: PreviewAnim,
}

/// Every entry the walk will reach, resolved through the same generated table
/// the walk and the renderers use, plus the one catalog rendered through a
/// document. Nothing here carries a list of catalogs or of entries.
fn previewable() -> Vec<Previewable> {
    let mechanisms = preview_mechanisms();
    let mut out = Vec::new();
    for cat in catalogs() {
        let previewable_entries = || cat.entries.iter().filter(|e| e.supports_preview);
        if let Some((id, mech)) = mechanisms.iter().find(|(id, _)| *id == cat.id) {
            for e in previewable_entries() {
                let entry = mech.resolve(e.type_id).unwrap_or_else(|| {
                    panic!(
                        "`{id}/{}` is previewable but resolves to nothing",
                        e.type_id
                    )
                });
                out.push(Previewable {
                    catalog: id,
                    type_id: entry.type_id,
                    anim: entry.anim,
                });
            }
            continue;
        }
        if cat.id == darkly::gpu::blend_mode::CATALOG_ID {
            for e in previewable_entries() {
                out.push(Previewable {
                    catalog: cat.id,
                    type_id: e.type_id,
                    anim: darkly::gpu::blend_mode::registry()
                        .preview(e.type_id)
                        .expect("a registered mode inherits the catalog preview"),
                });
            }
            continue;
        }
        if cat.id == darkly::brush::builtin_brushes::CATALOG_ID {
            for e in previewable_entries() {
                out.push(Previewable {
                    catalog: cat.id,
                    type_id: e.type_id,
                    anim: darkly::brush::builtin_brushes::preview(e.type_id)
                        .expect("a shipped brush inherits the catalog preview"),
                });
            }
            continue;
        }
        assert!(
            previewable_entries().next().is_none(),
            "catalog `{}` has previewable entries and no renderer",
            cat.id
        );
    }
    assert!(!out.is_empty(), "no previewable entries found at all");
    out
}

// ---------------------------------------------------------------------------
// The declarations — GPU-free
// ---------------------------------------------------------------------------

/// Every filter, every veil, every blend mode and `noise` declares a preview.
///
/// Driven off the four **registries** rather than a hand-written list, so
/// adding a filter without a preview fails here — which is the whole point of
/// putting the declaration on the registration.
#[test]
fn every_previewable_entry_declares_a_preview() {
    let filters = darkly::gpu::filter::FilterPipelineRegistry::new();
    for reg in filters.types() {
        assert!(
            filters.preview(reg.type_id).is_some(),
            "filter `{}` declares no preview",
            reg.type_id
        );
    }
    let veils = darkly::gpu::veil::VeilRegistry::new();
    for reg in veils.types() {
        assert!(
            veils.preview(reg.type_id).is_some(),
            "veil `{}` declares no preview",
            reg.type_id
        );
    }
    for reg in darkly::gpu::blend_mode::registry().all() {
        assert!(
            darkly::gpu::blend_mode::registry()
                .preview(reg.type_id)
                .is_some(),
            "blend mode `{}` declares no preview",
            reg.type_id
        );
    }
    assert!(darkly::gpu::void::VoidRegistry::new()
        .preview("noise")
        .is_some());
}

/// The two catalogs with previewable entries and no `src → out` mechanism.
///
/// A blend mode is a relation between two images rather than an effect over
/// one; a brush is a stroke driven through the brush engine rather than an
/// effect over one image. Neither has a pass to open, so each is rendered by its
/// own arm in `render_entry` — as a further *caller* of `PreviewAnim`, not a
/// further preview system.
const MECHANISMLESS: [&str; 2] = [
    darkly::gpu::blend_mode::CATALOG_ID,
    darkly::brush::builtin_brushes::CATALOG_ID,
];

/// Every previewable catalog has a mechanism, or is one of the documented
/// exceptions.
///
/// The generated table is what both consumers dispatch through, so an entry it
/// cannot reach has no picker preview however much it declares.
#[test]
fn every_previewable_catalog_has_a_mechanism_or_is_the_exception() {
    let mechanisms = preview_mechanisms();
    for (id, _) in &mechanisms {
        assert!(
            catalogs().iter().any(|c| c.id == *id),
            "`{id}` is not a catalog id"
        );
    }
    for cat in catalogs() {
        if !cat.entries.iter().any(|e| e.supports_preview) {
            continue;
        }
        let has = mechanisms.iter().any(|(id, _)| *id == cat.id);
        assert_eq!(
            has,
            !MECHANISMLESS.contains(&cat.id),
            "`{}` previewability and mechanism disagree",
            cat.id
        );
    }
}

/// The two effects with two surfaces declare one preview and sweep one set of
/// values, from the module they share rather than a copy in each.
///
/// The filter half is checked structurally — the registration's swept values
/// *are* the shared function's. The veil half calls the same function from its
/// `preview_at`, which only pixels can witness; `every_asset_has_real_motion`
/// and `preview_at_is_absolute` cover it there.
#[test]
fn shared_effects_share_one_preview() {
    let filters = darkly::gpu::filter::FilterPipelineRegistry::new();
    let veils = darkly::gpu::veil::VeilRegistry::new();
    for shared in ["black_and_white", "chromatic_aberration"] {
        assert_eq!(
            filters.preview(shared),
            veils.preview(shared),
            "`{shared}`'s filter and veil declare different previews"
        );
    }
    for i in 0..8 {
        let t = i as f32 / 8.0;
        assert_eq!(
            filters.preview_params("black_and_white", t),
            darkly::gpu::black_and_white::preview_params(t),
        );
        assert_eq!(
            filters.preview_params("chromatic_aberration", t),
            darkly::gpu::filters::chromatic_aberration::preview_params(t),
        );
    }
}

/// A void's previewability *is* its declaration — one fact, not two that can
/// drift.
#[test]
fn void_previewability_is_the_declaration() {
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

/// Every declared animation is playable: a positive frame count and a positive
/// rate. A zero of either would divide by zero on the playback clock.
#[test]
fn every_declared_animation_is_playable() {
    for p in previewable() {
        assert!(
            p.anim.frames >= 1,
            "`{}/{}` declares no frames",
            p.catalog,
            p.type_id
        );
        assert!(
            p.anim.fps >= 1,
            "`{}/{}` declares a zero playback rate",
            p.catalog,
            p.type_id
        );
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

/// Seven filters, nine veils, one void, sixteen blend modes and thirteen brushes
/// — counted **per catalog**. A bare total of forty-six would not notice a
/// whole catalog dropping out and another gaining entries.
#[test]
fn all_forty_six_assets_land() {
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
            ("veils", 9),
            ("voids", 1),
            ("blendModes", 16),
            ("brushes", 13),
        ])
    );
    assert_eq!(counts.values().sum::<usize>(), 46);
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

/// Every written PNG decodes to the size its index entry declares, and every
/// effect asset is the fixed square subject.
///
/// Pins the coincidence the layout depends on: the offscreen veil and void
/// renderers are hard-wired to fit into the picker's preview box, and the
/// document path is sized to match it. If that constant ever moved, veil and
/// void frames would silently diverge in size from the rest. A brush stroke is
/// the one asset that is deliberately not square — it is a left-to-right line
/// framed to the picker strip's own shape, so it is checked against that
/// constant instead.
#[test]
fn every_frame_is_the_size_its_entry_declares() {
    let (dir, manifest) = assets();
    let dim = darkly::docs_render::subject::DOCS_SUBJECT_DIM;

    let mut checked = 0usize;
    for (catalog, entries) in &manifest.assets {
        let expected = if catalog == darkly::brush::builtin_brushes::CATALOG_ID {
            darkly::engine::BRUSH_THUMBNAIL_SIZE
        } else {
            (dim, dim)
        };
        for asset in entries.values() {
            assert_eq!((asset.width, asset.height), expected, "{}", asset.dir);
            let path = dir.join(&asset.dir).join("000.png");
            let img = image::open(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            assert_eq!(
                (img.width(), img.height()),
                expected,
                "{} decodes to a different size than it declares",
                asset.dir
            );
            checked += 1;
        }
    }
    assert_eq!(checked, 46);
}

/// For every asset the PNG count equals the frame count the declaration says
/// will be *emitted*, and the index's `frames` / `fps` / `loop` are the entry's
/// own — which is what makes `loop` in the artifact something a consumer can
/// rely on rather than a claim.
///
/// Emitted, not declared: `close_loop` spends `LOOP_CLOSE_FRAMES` of a one-way
/// sequence on its own hand-back, so what a consumer holds is shorter than what
/// was rendered, and `loop` is then true because the frames really do close.
#[test]
fn manifest_frames_fps_and_loop_match_the_declaration() {
    let (dir, manifest) = assets();
    let mut non_looping = BTreeSet::new();

    for p in previewable() {
        let asset = &manifest.assets[p.catalog][p.type_id];
        assert_eq!(
            asset.frames,
            p.anim.emitted_frames(),
            "{}/{}",
            p.catalog,
            p.type_id
        );
        assert_eq!(asset.fps, p.anim.fps, "{}/{}", p.catalog, p.type_id);
        assert_eq!(
            asset.loops,
            p.anim.emits_a_loop(),
            "{}/{}",
            p.catalog,
            p.type_id
        );
        assert_eq!(
            asset.still,
            p.anim.still_frame(),
            "{}/{}",
            p.catalog,
            p.type_id
        );
        assert!(
            asset.still < asset.frames,
            "`{}/{}`'s poster frame falls outside its own sequence",
            p.catalog,
            p.type_id
        );
        assert!(
            dir.join(&asset.dir)
                .join(format!("{:03}.png", asset.still))
                .exists(),
            "`{}/{}`'s poster frame names no file",
            p.catalog,
            p.type_id
        );

        let written = std::fs::read_dir(dir.join(&asset.dir)).unwrap().count();
        assert_eq!(
            written as u32,
            p.anim.emitted_frames(),
            "`{}/{}` wrote {written} frames",
            p.catalog,
            p.type_id
        );
        if !p.anim.loops {
            non_looping.insert(format!("{}/{}", p.catalog, p.type_id));
        }
    }

    // The three time-driven veils integrate their clocks forward and declare so
    // rather than being made periodic by a shader change. They still ship as
    // loops: `close_loop` closes the sequence in the one place both the picker
    // and this binary go through, so nothing downstream carries a special case.
    assert_eq!(
        non_looping,
        BTreeSet::from([
            "veils/grain".to_string(),
            "veils/rainy_glass".to_string(),
            "veils/vhs".to_string(),
        ])
    );
    for id in &non_looping {
        let (catalog, type_id) = id.split_once('/').unwrap();
        assert!(
            manifest.assets[catalog][type_id].loops,
            "`{id}` declares one-way motion but was not closed into a loop"
        );
    }

    // Alphabetically the last of the six tests that read the fixture, and under
    // the mandatory single test thread that makes it the last to run — so the
    // 65–160 MB of frames come back here rather than being left on every
    // developer machine and every CI run. The fixture also clears any earlier
    // tree before writing, so a run that panicked or was killed reclaims its
    // predecessor's space rather than adding to it.
    std::fs::remove_dir_all(dir).expect("the fixture cleans up after itself");
}

/// Every asset that declares more than one frame renders at least two distinct
/// images; one that declares a still writes exactly one file.
///
/// Motion is a method now, so there is no declaration left to inspect — the
/// pixels are the whole of the evidence, and this is the floor.
/// `tests/picker_preview.rs` carries the finer-grained assertions over the same
/// driver.
#[test]
fn every_asset_has_real_motion() {
    let (dir, manifest) = assets();
    for entries in manifest.assets.values() {
        for (type_id, asset) in entries {
            let frame = |i: u32| std::fs::read(dir.join(&asset.dir).join(format!("{i:03}.png")));
            if asset.frames == 1 {
                assert!(
                    frame(1).is_err(),
                    "`{type_id}` declares a still and wrote more"
                );
                continue;
            }
            let first = frame(0).unwrap();
            let moved = (1..asset.frames).any(|i| frame(i).map(|f| f != first).unwrap_or(false));
            assert!(
                moved,
                "`{}` rendered {} identical frames",
                type_id, asset.frames
            );
        }
    }
}

/// Rendering an entry twice through the same `Gpu` produces the same pixels.
///
/// Determinism across a reused device is what lets forty-six assets share one
/// `Gpu`, and it is where a renderer that left state behind shows up. One entry
/// per catalog rather than all forty-six, because the cost is two full
/// sequences each and the failure mode is per-renderer, not per-entry — except
/// for brushes, which get
/// [`every_brush_renders_the_same_bytes_twice`] over the whole catalog.
#[test]
fn rendering_an_entry_twice_is_deterministic() {
    let mut gpu = Gpu::new();
    for (catalog, type_id) in [
        ("filters", "hsv"),
        ("veils", "frozen"),
        ("voids", "noise"),
        ("blendModes", "multiply"),
        ("brushes", "ink_pen"),
    ] {
        let first = render_one(&mut gpu, catalog, type_id);
        let again = render_one(&mut gpu, catalog, type_id);
        assert_eq!(
            first.frames, again.frames,
            "`{catalog}/{type_id}` rendered two different sequences"
        );
    }
}

/// Value-pinned, not merely "it differs": `invert`'s single frame is exactly
/// `255 - c` per RGB channel of the source the target loaded.
///
/// Compared against the target's *loaded* source rather than `subject_rgba`,
/// because the offscreen path area-averages the 2× subject before the filter
/// sees it — pinning the raw subject would be pinning the resample. Rendered
/// after every other filter through the same session, so it also pins that none
/// of them left state behind.
#[test]
fn invert_is_the_exact_inverse_of_the_source_it_was_given() {
    let mut gpu = Gpu::new();
    let mut rendered = None;
    for reg in darkly::gpu::filter::FilterPipelineRegistry::new().types() {
        let r = render_one(&mut gpu, "filters", reg.type_id);
        if reg.type_id == "invert" {
            rendered = Some(r);
        }
    }
    let rendered = rendered.expect("invert is registered");
    assert_eq!(rendered.frames.len(), 1, "invert declares a still");

    let source = docs_render::test_source_pixels(&mut gpu);
    let frame = &rendered.frames[0];
    assert_eq!(frame.len(), source.len());
    for (i, (out, src)) in frame
        .as_chunks::<4>()
        .0
        .iter()
        .zip(source.as_chunks::<4>().0)
        .enumerate()
    {
        assert_eq!(
            [out[0], out[1], out[2], out[3]],
            [255 - src[0], 255 - src[1], 255 - src[2], src[3]],
            "pixel {i} is not the exact inverse of the source"
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
    // Frame 0 is where the shared sweep rests: no tint, so the result is the
    // bare desaturation.
    let defs = darkly::gpu::veil::VeilRegistry::new().param_defs("black_and_white");
    let tint = defs
        .iter()
        .position(|d| d.name == "tint_strength")
        .expect("the shared schema declares a tint strength");
    assert_eq!(
        darkly::gpu::black_and_white::preview_params(0.0)[tint],
        ParamValue::Float(0.0),
        "the shared sweep starts untinted"
    );

    let rendered = render_one(&mut Gpu::new(), "veils", "black_and_white");
    let frame = &rendered.frames[0];
    for (i, px) in frame.as_chunks::<4>().0.iter().enumerate() {
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
            frame.as_chunks::<4>().0.iter().any(|px| px != first),
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

    // Half way through the sweep, where the blended layer fully covers the
    // backdrop. Read off the same closure the renderer drives, so the index and
    // the motion cannot disagree.
    let anim = darkly::gpu::blend_mode::registry()
        .preview("normal")
        .unwrap();
    let full = (0..anim.frames)
        .max_by(|a, b| {
            let at = |i: u32| docs_render::blend_opacity_at(frame_t(i, anim.frames));
            at(*a).total_cmp(&at(*b))
        })
        .expect("the sweep has frames");

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

/// **Every** brush renders the same bytes twice — all thirteen, not a sample.
///
/// Unlike the other catalogs the failure mode here *is* per-entry: `rough_ink`,
/// `rough_watercolor` and `smooth_watercolor` contain `random`/`noise` nodes and
/// are the only entries in the artifact that can fail this, so a test naming one
/// of the other ten would go green while the property was false for three.
/// A documentation artifact that rewrites bytes with no change of meaning churns
/// on every rebuild, which is what the preview stroke seed being a constant
/// rather than a clock read prevents.
#[test]
fn every_brush_renders_the_same_bytes_twice() {
    let mut gpu = Gpu::new();
    let catalog = darkly::brush::builtin_brushes::CATALOG_ID;
    let mut checked = 0;
    for entry in darkly::brush::builtin_brushes::catalog().entries {
        let first = render_one(&mut gpu, catalog, entry.type_id);
        let again = render_one(&mut gpu, catalog, entry.type_id);
        assert_eq!(
            first.frames, again.frames,
            "`{}` renders differently every time",
            entry.type_id
        );
        checked += 1;
    }
    assert_eq!(checked, 13, "the shipped brush set");
}

/// Every brush asset is one frame, and that frame shows a stroke.
///
/// The documentation counterpart of `brush_preview_staging.rs`'s regression: the
/// four content-dependent brushes baked a flat rectangle before the backdrop was
/// staged under them, and a flat rectangle is not documentation.
#[test]
fn every_brush_asset_shows_a_stroke() {
    let (dir, manifest) = assets();
    let entries = &manifest.assets[darkly::brush::builtin_brushes::CATALOG_ID];
    assert_eq!(entries.len(), 13);

    for (type_id, asset) in entries {
        assert_eq!(asset.frames, 1, "`{type_id}` is a still");
        let img = image::open(dir.join(&asset.dir).join("000.png"))
            .unwrap_or_else(|e| panic!("{}: {e}", asset.dir))
            .to_rgba8();
        let lums: Vec<f32> = img
            .pixels()
            .map(|p| 0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32)
            .collect();
        let mean = lums.iter().sum::<f32>() / lums.len() as f32;
        let sd = (lums.iter().map(|l| (l - mean).powi(2)).sum::<f32>() / lums.len() as f32).sqrt();
        assert!(
            sd > 12.0,
            "`{type_id}` documents a flat rectangle (SD {sd:.2})"
        );
    }
}
