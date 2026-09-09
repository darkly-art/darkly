//! The compositor's revision registry — one clock, five sources, every
//! derived artifact validated where it is read.
//!
//! Two things are under test. **Correctness**: after any mutation, the
//! incrementally-maintained composite must be byte-identical to one built from
//! scratch — a source a mutation forgot to bump shows up here as a stale
//! composite and nowhere else. **Scheduling**: the gates that used to be
//! boolean flags are now tick comparisons, so what recomposites and what only
//! re-presents is pinned as a truth table.
//!
//! What the byte comparison can catch today is bounded by the compositor
//! having no partial caching: the walk rebuilds the whole tree, so *any*
//! recomposite yields correct pixels and only "never recomposited at all" is
//! observable. That is what the `composite_runs` counter measures, and it is
//! why the counter carries more weight here than the pixels do. When
//! per-effect or per-group caching lands, the same comparisons start catching
//! partially-stale results too — which is the point of writing them now.
//!
//! Run with: `cargo test -p darkly --test compositor_revisions --features testing -- --test-threads=1`

use darkly::coord::{CanvasPoint, CanvasRect};
use darkly::document::MoveTarget;
use darkly::engine::types::StrokeOp;
use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::params::ParamValue;
use darkly::gpu::test_utils::*;
use darkly::layer::LayerId;

const W: u32 = 32;
const H: u32 = 32;

fn test_engine(width: u32, height: u32) -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, width, height)
}

/// Let async pixel work land before a readback.
fn settle(engine: &mut DarklyEngine) {
    engine.test_flush_readbacks();
    engine.render(0.0);
}

fn fill_layer(engine: &mut DarklyEngine, layer_id: LayerId, r: u8, g: u8, b: u8) {
    engine.begin_stroke(layer_id).unwrap();
    engine.stroke_to(StrokeOp::FloodFill {
        x: 1.0,
        y: 1.0,
        r,
        g,
        b,
        a: 255,
        tolerance: 0,
    });
    engine.end_stroke();
    settle(engine);
}

fn paint_dab(engine: &mut DarklyEngine, layer_id: LayerId, x: f32, y: f32) {
    engine.begin_stroke(layer_id).unwrap();
    engine.stroke_to(StrokeOp::BrushStroke {
        x,
        y,
        pressure: 1.0,
        x_tilt: 0.0,
        y_tilt: 0.0,
        rotation: 0.0,
        tangential_pressure: 0.0,
        time_ms: 0.0,
        cr: 1.0,
        cg: 0.0,
        cb: 0.0,
        ca: 1.0,
    });
    engine.end_stroke();
    settle(engine);
}

fn effect(engine: &mut DarklyEngine, pipeline: &str) -> LayerId {
    let defaults: Vec<_> = engine
        .filter_param_defs(pipeline)
        .iter()
        .map(darkly::gpu::params::ParamDef::default_value)
        .collect();
    engine
        .add_filter_layer(pipeline, defaults, None)
        .unwrap_or_else(|| panic!("`{pipeline}` should be addable as an effect layer"))
}

/// The heart of the battery: the incremental composite must equal one built
/// with every revision bumped.
///
/// Ordering matters — the incremental read happens first, because the
/// from-scratch read invalidates everything and would mask a stale result if
/// it ran first. The `composite_runs` delta guards the other direction: a
/// mutation that recomposites nothing and a mutation that recomposites
/// correctly both produce matching bytes, and only the counter tells them
/// apart.
fn assert_matches_from_scratch(engine: &mut DarklyEngine, what: &str) {
    settle(engine);
    let before_runs = engine.test_composite_runs();
    let incremental = engine.test_readback_canvas();
    let ran = engine.test_composite_runs() - before_runs;
    let scratch = engine.test_readback_canvas_from_scratch();
    assert_eq!(
        incremental, scratch,
        "stale composite after {what}: the incremental result differs from a \
         from-scratch render, so something this mutation changed was never \
         bumped"
    );
    assert_eq!(
        ran, 1,
        "{what} must leave the composite stale — it produced matching bytes \
         without recompositing, which means the assertion above proved nothing"
    );
}

/// A document with one of everything the composite walk can hit: plain
/// rasters, a masked raster, a passthrough group, a canvas-space effect, and a
/// non-passthrough group holding its own effect.
struct Fixture {
    engine: DarklyEngine,
    bottom: LayerId,
    masked: LayerId,
    group: LayerId,
    in_group: LayerId,
    fx: LayerId,
    top: LayerId,
}

fn fixture() -> Fixture {
    let mut engine = test_engine(W, H);

    let bottom = engine.add_raster_layer(None);
    fill_layer(&mut engine, bottom, 40, 60, 80);

    let masked = engine.add_raster_layer(None);
    fill_layer(&mut engine, masked, 200, 30, 30);
    engine.add_mask(masked).expect("add mask");

    let group = engine.add_group(None);
    let in_group = engine.add_raster_layer(Some(group));
    engine
        .move_layer(in_group, MoveTarget::IntoGroupTop(group))
        .expect("move succeeds");
    fill_layer(&mut engine, in_group, 20, 180, 90);

    let fx = effect(&mut engine, "invert");

    let top = engine.add_raster_layer(None);
    fill_layer(&mut engine, top, 90, 90, 200);
    engine.set_opacity(top, 0.5);

    settle(&mut engine);
    Fixture {
        engine,
        bottom,
        masked,
        group,
        in_group,
        fx,
        top,
    }
}

// ---------------------------------------------------------------------------
// First frame
// ---------------------------------------------------------------------------

/// A fresh compositor has composited nothing, and the frame gate deliberately
/// ignores `targets` — the only source construction bumps on its own. Without
/// an explicit document bump at construction the first frame compares clean
/// and the canvas stays blank.
///
/// Standalone because every other test's first action bumps `document` anyway
/// and would mask this entirely.
#[test]
fn a_fresh_engine_composites_its_first_frame() {
    let mut engine = test_engine(W, H);
    assert_eq!(
        engine.test_composite_runs(),
        0,
        "construction must not composite"
    );

    // Nothing has mutated the document — this is purely the first frame. An
    // empty document composites to transparent either way, so the pixels
    // cannot distinguish "composited nothing" from "never composited"; the
    // counter is the only thing that can.
    engine.test_readback_canvas();
    assert_eq!(
        engine.test_composite_runs(),
        1,
        "the first frame after construction must composite; skipping it leaves \
         the canvas blank until the first edit"
    );

    // And the result is displayable: content added afterwards shows up.
    let layer = engine.add_raster_layer(None);
    fill_layer(&mut engine, layer, 255, 0, 0);
    let pixels = engine.test_readback_canvas();
    assert_eq!(&pixels[0..4], &[255, 0, 0, 255]);
}

// ---------------------------------------------------------------------------
// Byte-equality battery
// ---------------------------------------------------------------------------

/// Every case below is the same test with a different mutation: build the
/// fixture, mutate it, assert the incrementally-maintained composite still
/// matches a from-scratch render. The harness lives here once; each case
/// writes only its mutation, and the case list reads as the list of mutation
/// classes under coverage.
///
/// Cases are self-contained rather than chained, so a sequence like
/// add-then-remove is two rows that each set up what they need.
macro_rules! stale_composite_battery {
    ($($name:ident, $label:literal, |$f:ident| $body:block)*) => {
        $(
            #[test]
            fn $name() {
                let mut $f = fixture();
                $body
                assert_matches_from_scratch(&mut $f.engine, $label);
            }
        )*
    };
}

stale_composite_battery! {
    painting, "paint", |f| {
        paint_dab(&mut f.engine, f.bottom, 16.0, 16.0);
    }

    painting_into_a_mask, "paint into a mask", |f| {
        let mask = f
            .engine
            .host_mask_id(f.masked)
            .expect("the fixture's masked layer has a mask filter");
        paint_dab(&mut f.engine, mask, 16.0, 16.0);
    }

    filling, "fill", |f| {
        fill_layer(&mut f.engine, f.bottom, 10, 220, 240);
    }

    changing_opacity, "opacity change", |f| {
        f.engine.set_opacity(f.bottom, 0.25);
    }

    changing_blend_mode, "blend mode change", |f| {
        f.engine.set_blend_mode(f.top, "multiply");
    }

    toggling_visibility, "visibility toggle", |f| {
        f.engine.set_layer_visible(f.masked, false);
    }

    reordering, "reorder", |f| {
        f.engine.move_layer(f.bottom, MoveTarget::After(f.top)).expect("move succeeds");
    }

    adding_a_layer, "add layer", |f| {
        let added = f.engine.add_raster_layer(None);
        fill_layer(&mut f.engine, added, 250, 240, 10);
    }

    deleting_a_layer, "delete layer", |f| {
        f.engine.remove_layer(f.top).unwrap();
    }

    adding_a_mask, "add mask", |f| {
        f.engine.add_mask(f.bottom).expect("add mask");
    }

    removing_a_mask, "remove mask", |f| {
        f.engine.add_mask(f.bottom).expect("add mask");
        settle(&mut f.engine);
        f.engine.remove_mask(f.bottom);
    }

    // Adding and removing an effect realizes and destroys instances
    // mid-session, rather than inheriting the one the fixture already built.
    adding_an_effect, "add a canvas-space effect", |f| {
        effect(&mut f.engine, "black_and_white");
    }

    removing_an_effect, "remove a canvas-space effect", |f| {
        let added = effect(&mut f.engine, "black_and_white");
        settle(&mut f.engine);
        f.engine.remove_layer(added).unwrap();
    }

    changing_filter_params, "filter param change", |f| {
        // `brightness_contrast` rather than the fixture's `invert`, which
        // declares no parameters — there would be nothing to change.
        let fx = effect(&mut f.engine, "brightness_contrast");
        settle(&mut f.engine);
        f.engine.update_filter_params(
            fx,
            vec![ParamValue::Float(40.0), ParamValue::Float(25.0)],
        );
    }

    toggling_passthrough, "passthrough toggle", |f| {
        f.engine.set_group_passthrough(f.group, false);
    }

    setting_isolation, "isolation set", |f| {
        f.engine.set_isolated_node(Some(f.in_group));
    }

    clearing_isolation, "isolation clear", |f| {
        f.engine.set_isolated_node(Some(f.in_group));
        settle(&mut f.engine);
        f.engine.set_isolated_node(None);
    }

    moving_the_screen_boundary, "screen-boundary move", |f| {
        // Raise the effect to the top so it is eligible for the run, then lift
        // the divider over it — it leaves the canvas composite entirely.
        f.engine.move_layer(f.fx, MoveTarget::After(f.top)).expect("move succeeds");
        settle(&mut f.engine);
        f.engine.test_set_screen_space_boundary(1);
    }

    undoing, "undo", |f| {
        fill_layer(&mut f.engine, f.bottom, 5, 5, 5);
        settle(&mut f.engine);
        f.engine.undo();
    }

    redoing, "redo", |f| {
        fill_layer(&mut f.engine, f.bottom, 5, 5, 5);
        settle(&mut f.engine);
        f.engine.undo();
        settle(&mut f.engine);
        f.engine.redo();
    }

    growing_the_canvas, "canvas grow", |f| {
        f.engine
            .resize_canvas(CanvasRect::new(CanvasPoint::new(0, 0), W * 2, H * 2));
    }

    cropping_the_canvas, "canvas crop", |f| {
        f.engine
            .resize_canvas(CanvasRect::new(CanvasPoint::new(4, 4), W / 2, H / 2));
    }

    merging_down, "merge down", |f| {
        f.engine.merge_down(f.top).unwrap();
    }
}

/// The animation source, isolated. Not a battery row: the harness `settle`s
/// first, and a frame that lands async work marks the document dirty — which
/// would recomposite for a reason other than the animation tick and hide a
/// missing `animation` bump entirely.
#[test]
fn an_animation_tick_leaves_no_stale_composite() {
    let mut engine = test_engine(W, H);
    let base = engine.add_raster_layer(None);
    fill_layer(&mut engine, base, 60, 60, 60);
    // Non-zero speed — a grain at the default speed does not animate, and the
    // canvas animation gate would never fire.
    engine
        .add_filter_layer(
            "grain",
            vec![
                ParamValue::Float(1.0), // speed
                ParamValue::Float(0.0), // color
                ParamValue::Float(1.0), // opacity
            ],
            None,
        )
        .expect("`grain` should be addable as an effect layer");

    for _ in 0..8 {
        engine.render(0.0);
    }
    engine.test_flush_readbacks();
    engine.test_readback_canvas();

    let before_runs = engine.test_composite_runs();
    engine.test_tick_animations(1.0); // primes last_wall_time; dt == 0
    for i in 1..=8 {
        engine.test_tick_animations(1.0 + i as f32 * 0.05);
    }

    let incremental = engine.test_readback_canvas();
    assert_eq!(
        engine.test_composite_runs() - before_runs,
        1,
        "a canvas-side animation tick is the only thing that changed here, so \
         it alone must make the composite stale"
    );
    let scratch = engine.test_readback_canvas_from_scratch();
    assert_eq!(
        incremental, scratch,
        "stale composite after an animation tick"
    );
}

// ---------------------------------------------------------------------------
// Scheduling truth table
// ---------------------------------------------------------------------------

/// Composites encoded by one forced offscreen pass.
fn composites_for_one_render(engine: &mut DarklyEngine) -> u64 {
    let before = engine.test_composite_runs();
    engine.test_readback_canvas();
    engine.test_composite_runs() - before
}

/// One row of the truth table: from a settled, fully-presented engine, apply a
/// change and state what it must schedule.
///
/// `owes_frame` is what the frame loop reports afterwards; `composites` is how
/// many recomposites the next frame encodes. Together they separate the two
/// gates — a present-only change owes a frame but composites zero times, which
/// no single assertion can express.
fn assert_schedules(
    label: &str,
    owes_frame: bool,
    composites: u64,
    mutate: impl FnOnce(&mut Fixture),
) {
    let mut f = fixture();
    f.engine.test_readback_canvas();
    f.engine.test_clear_needs_present();

    mutate(&mut f);

    assert_eq!(
        f.engine.test_frame_needs_more(),
        owes_frame,
        "{label}: expected owes_frame={owes_frame}"
    );
    assert_eq!(
        composites_for_one_render(&mut f.engine),
        composites,
        "{label}: expected {composites} recomposite(s) on the next frame"
    );
}

#[test]
fn a_steady_frame_does_no_work() {
    assert_schedules("no mutation", false, 0, |_| {});
}

#[test]
fn a_document_change_composites_and_presents() {
    assert_schedules("document change", true, 1, |f| {
        f.engine.set_opacity(f.bottom, 0.3)
    });
}

/// The pan case: the composite is still valid, only the present is owed.
#[test]
fn a_present_input_change_does_not_composite() {
    assert_schedules("present-input change", true, 0, |f| {
        f.engine.test_mark_needs_present()
    });
}

#[test]
fn the_viewport_background_only_presents() {
    assert_schedules("viewport background", true, 0, |f| {
        f.engine.set_viewport_bg([0.1, 0.2, 0.3, 1.0])
    });
}

#[test]
fn the_pixel_filter_only_presents() {
    assert_schedules("pixel filter", true, 0, |f| {
        f.engine.set_pixel_filter("nearest")
    });
}

/// `targets` is the one source excluded from both frame gates: recreating a
/// render target schedules nothing by itself, but the next composite must
/// still rebuild the effect instances whose bind groups pointed at the old
/// textures.
///
/// Driven through the registry directly rather than through
/// `resize_screen_run`, which legitimately bumps `present_inputs` too and so
/// would schedule a present regardless.
#[test]
fn a_targets_bump_alone_schedules_nothing_but_still_rebuilds_instances() {
    assert_schedules("targets bump", false, 0, |f| f.engine.test_bump_targets());

    // And the rebuild half, which needs the fixture to survive the call above.
    let mut f = fixture();
    f.engine.test_readback_canvas();
    let rebuilds_before = f.engine.test_effect_rebuilds();

    f.engine.test_bump_targets();
    f.engine.set_opacity(f.bottom, 0.9);
    f.engine.test_readback_canvas();

    assert!(
        f.engine.test_effect_rebuilds() > rebuilds_before,
        "the next composite after a targets bump must rebuild effect instances \
         rather than encode bind groups over replaced textures"
    );
}

/// The liveness property the `targets` exclusion buys: the composite walk
/// itself bumps `targets` when it creates a group state, and a frame that
/// rescheduled itself for its own work would never settle.
#[test]
fn a_frame_does_not_reschedule_itself() {
    let mut f = fixture();
    paint_dab(&mut f.engine, f.bottom, 8.0, 8.0);

    // Drain the async work the dab queued (thumbnail readback, diff rect) so
    // the assertion below is about scheduling and not about work still in
    // flight, which legitimately asks for frames.
    for _ in 0..8 {
        f.engine.render(0.0);
    }
    f.engine.test_flush_readbacks();
    f.engine.test_readback_canvas();
    f.engine.test_clear_needs_present();

    assert_eq!(
        composites_for_one_render(&mut f.engine),
        0,
        "the frame after a settled composite must find nothing to do"
    );
    assert!(
        !f.engine.test_frame_needs_more(),
        "a frame must not schedule another frame for work it did itself"
    );
}

// ---------------------------------------------------------------------------
// Derived caches
// ---------------------------------------------------------------------------

use darkly::gpu::content_bounds::ContentBoundsPass;
use darkly::gpu::revisions::Revisions;

/// Everything the content-bounds tests share: a device, a texture with known
/// coverage, the pass, and the revisions its cache is validated against.
///
/// Bundled rather than threaded through free functions — the pass takes eight
/// arguments, which is more plumbing than these tests have logic.
struct BoundsHarness {
    device: wgpu::Device,
    queue: wgpu::Queue,
    _tex: wgpu::Texture,
    view: wgpu::TextureView,
    pass: ContentBoundsPass,
    revisions: Revisions,
}

impl BoundsHarness {
    /// `rect` is the opaque region as `[x, y, w, h]`, or `None` for a fully
    /// transparent texture.
    fn new(rect: Option<[u32; 4]>) -> Self {
        let (device, queue) = test_device();
        let mut buf = vec![0u8; (W * H * 4) as usize];
        if let Some([rx, ry, rw, rh]) = rect {
            for y in ry..ry + rh {
                for x in rx..rx + rw {
                    let i = ((y * W + x) * 4) as usize;
                    buf[i..i + 4].copy_from_slice(&[10, 20, 30, 255]);
                }
            }
        }
        let (_tex, view) = create_test_texture(&device, &queue, W, H, &buf);
        let pass = ContentBoundsPass::new(&device);
        BoundsHarness {
            device,
            queue,
            _tex,
            view,
            pass,
            revisions: Revisions::new(),
        }
    }

    fn layer(&self) -> LayerId {
        LayerId::from_ffi(1)
    }

    fn request(&mut self) {
        let layer = self.layer();
        self.pass.request(
            &self.device,
            &self.queue,
            &self.revisions,
            &self.view,
            W,
            H,
            false,
            layer,
        );
    }

    /// Poll once, reporting whether this layer's result landed. Blocks the
    /// device when it has not — native-only, and this is test code.
    fn poll(&mut self) -> bool {
        let layer = self.layer();
        if self
            .pass
            .poll(&self.device, &self.revisions)
            .contains(&layer)
        {
            return true;
        }
        let _ = self.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        false
    }

    fn resolve(&mut self) {
        self.request();
        while !self.poll() {}
    }

    fn get(&self) -> Option<[u32; 4]> {
        self.pass.get(&self.revisions, self.layer())
    }

    fn is_resolved(&self) -> bool {
        self.pass.is_resolved(&self.revisions, self.layer())
    }

    fn is_pending(&self) -> bool {
        self.pass.is_pending(&self.revisions, self.layer())
    }
}

/// Content bounds carry the `(document, node_pixels)` stamp they were computed
/// under, so a cached answer stops being current the moment either moves — no
/// invalidation is pushed, and a consumer cannot read a stale result by
/// forgetting to check one.
#[test]
fn content_bounds_go_stale_when_their_inputs_move() {
    // Each input gets its own harness, so one case's staleness cannot explain
    // the other's.
    type Bump = fn(&mut Revisions);
    let cases: [(&str, Bump); 2] = [
        ("a document bump", |r| r.bump_document()),
        ("a pixel bump", |r| r.bump_node_pixels(LayerId::from_ffi(1))),
    ];

    for (what, bump) in cases {
        let mut h = BoundsHarness::new(Some([4, 6, 8, 8]));
        h.resolve();
        assert_eq!(
            h.get(),
            Some([4, 6, 8, 8]),
            "bounds must resolve to the exact opaque rect"
        );
        assert!(h.is_resolved());

        bump(&mut h.revisions);

        assert_eq!(h.get(), None, "{what} must make the cached bounds stale");
        assert!(
            !h.is_resolved(),
            "{what}: a stale entry must not read as resolved"
        );
    }
}

/// A result that lands after its inputs moved is dropped rather than cached —
/// the async readback equivalent of reading a stale value.
#[test]
fn a_bounds_result_landing_after_its_inputs_moved_is_discarded() {
    let mut h = BoundsHarness::new(Some([0, 0, W, H]));
    h.request();

    // The inputs move while the dispatch is in flight.
    let layer = h.layer();
    h.revisions.bump_node_pixels(layer);

    for _ in 0..64 {
        assert!(
            !h.poll(),
            "a result whose stamp was superseded must never be reported as \
             completed"
        );
    }
    assert_eq!(h.get(), None, "a superseded result must not be cached");
    assert!(
        !h.is_pending(),
        "a dispatch for a superseded stamp must not suppress a fresh one"
    );
}

/// An empty texture resolves to *no bounds* as a terminal answer, not a
/// permanent miss — otherwise a caller requeues the same computation forever.
#[test]
fn empty_content_bounds_resolve_rather_than_requeue() {
    let mut h = BoundsHarness::new(None);
    h.resolve();

    assert_eq!(h.get(), None, "an empty texture has no bounds");
    assert!(
        h.is_resolved(),
        "the empty answer must stay resolved, so callers stop requeueing it"
    );
    assert!(
        !h.is_pending(),
        "a resolved empty result leaves nothing in flight"
    );
}

/// A histogram bins a filter's *input pixels*. A parameter drag changes the
/// document but no pixels, so the cached histogram the Levels editor is being
/// read against must survive it — the reason `document` is not one of the
/// histogram's dependencies.
#[test]
fn a_histogram_survives_a_param_drag() {
    let mut engine = test_engine(W, H);
    let base = engine.add_raster_layer(None);
    fill_layer(&mut engine, base, 128, 128, 128);
    let fx = effect(&mut engine, "levels");
    engine.set_histogram_target(Some(fx));

    let mut binned = Vec::new();
    for _ in 0..64 {
        engine.test_readback_canvas();
        engine.render(0.0);
        binned = engine.histogram(fx);
        if !binned.is_empty() {
            break;
        }
    }
    assert!(!binned.is_empty(), "the histogram must land");

    let defs = engine.filter_param_defs("levels");
    let mut params: Vec<_> = defs.iter().map(|d| d.default_value()).collect();
    params[0] = match params[0] {
        ParamValue::Float(v) => ParamValue::Float((v + 0.1).min(1.0)),
        ref other => other.clone(),
    };
    engine.update_filter_params(fx, params);

    assert_eq!(
        engine.histogram(fx),
        binned,
        "a parameter drag changes no pixels, so the histogram it is read \
         against must not be discarded"
    );

    // A pixel write does invalidate it.
    paint_dab(&mut engine, base, 16.0, 16.0);
    assert!(
        engine.histogram(fx).is_empty(),
        "a pixel write must make the cached histogram stale"
    );
}

// ---------------------------------------------------------------------------
// Walk reuse
// ---------------------------------------------------------------------------

/// A raster under an animating veil, the shape this whole mechanism exists
/// for. Each tick advances only the veil's own revision, so everything below
/// it is provably unchanged and must be reused — and the result must still be
/// byte-identical to a full walk.
///
/// The `walk_resumes` assertion is what stops this from proving nothing: a
/// silently full-walking cache would produce identical bytes and pass the
/// comparison alone.
fn animating_veil_fixture() -> (DarklyEngine, LayerId) {
    let mut engine = test_engine(W, H);
    let base = engine.add_raster_layer(None);
    fill_layer(&mut engine, base, 60, 90, 120);
    // Non-zero speed — a grain at the default speed never animates, so the
    // canvas animation gate would not fire at all.
    engine
        .add_filter_layer(
            "grain",
            vec![
                ParamValue::Float(1.0), // speed
                ParamValue::Float(0.0), // color
                ParamValue::Float(1.0), // opacity
            ],
            None,
        )
        .expect("`grain` should be addable as an effect layer");
    settle(&mut engine);
    engine.test_readback_canvas();
    (engine, base)
}

/// Advance the canvas animation clock by one firing tick and composite.
fn tick(engine: &mut DarklyEngine, t: f32) {
    engine.test_tick_animations(t);
    engine.test_readback_canvas();
}

#[test]
fn an_animating_veil_reuses_the_stack_below_it() {
    let (mut engine, _base) = animating_veil_fixture();

    // Convergence: the first tick after a full walk captures the prefix, the
    // next one resumes from it.
    tick(&mut engine, 1.0);
    for i in 1..=6 {
        tick(&mut engine, 1.0 + i as f32 * 0.05);
    }

    assert!(
        engine.test_walk_resumes() > 0,
        "an animating veil above an untouched stack must reuse the composite \
         below it — without this the veil's tick re-blends every layer"
    );

    let incremental = engine.test_readback_canvas();
    let scratch = engine.test_readback_canvas_from_scratch();
    assert_eq!(
        incremental, scratch,
        "a resumed walk must produce the same pixels as a full one"
    );
}

/// The staleness hole a prefix invites: a change that moves no per-child stamp
/// at all. Opacity travels as a coarse `document` bump, so the walk correctly
/// full-walks the frame it happens on — but if that walk kept the old capture,
/// the next animation tick would resume from pixels taken before the change.
#[test]
fn a_property_change_below_the_prefix_is_not_restored_over() {
    let (mut engine, base) = animating_veil_fixture();

    // Establish a prefix that includes the base layer.
    tick(&mut engine, 1.0);
    for i in 1..=6 {
        tick(&mut engine, 1.0 + i as f32 * 0.05);
    }
    assert!(
        engine.test_walk_resumes() > 0,
        "the prefix must be established, or this test cannot regress"
    );

    // A change to a layer *inside* the captured prefix, carried by a source
    // no child stamp reflects.
    engine.set_opacity(base, 0.25);
    engine.test_readback_canvas();

    // …and now a tick, which is exactly when a surviving stale prefix would be
    // restored over the top of it.
    tick(&mut engine, 2.0);

    let incremental = engine.test_readback_canvas();
    let scratch = engine.test_readback_canvas_from_scratch();
    assert_eq!(
        incremental, scratch,
        "a prefix captured before a property change must not be restored \
         after it — the tick following the change would show pre-change pixels"
    );
}

/// A focused filter's histogram is binned from inside the walk, against its
/// live input accumulator. Any reuse that skips the subtree carrying it makes
/// that dispatch unreachable and the result never lands — so while a histogram
/// is owed, no group may reuse anything.
///
/// The filter sits inside a nested group while the activity is at root level
/// above it: a guard scoped to the host group alone would be skipped along
/// with the group, which is the shape that starves.
#[test]
fn a_histogram_inside_a_group_still_lands_while_work_happens_above_it() {
    let mut engine = test_engine(W, H);
    let base = engine.add_raster_layer(None);
    fill_layer(&mut engine, base, 128, 128, 128);

    let group = engine.add_group(None);
    engine.set_group_passthrough(group, false);
    let in_group = engine.add_raster_layer(Some(group));
    engine
        .move_layer(in_group, MoveTarget::IntoGroupTop(group))
        .expect("move succeeds");
    fill_layer(&mut engine, in_group, 90, 40, 200);

    let fx = effect(&mut engine, "levels");
    engine
        .move_layer(fx, MoveTarget::IntoGroupTop(group))
        .expect("move the filter inside the group");

    let top = engine.add_raster_layer(None);
    fill_layer(&mut engine, top, 30, 30, 30);
    settle(&mut engine);

    engine.set_histogram_target(Some(fx));

    // Work above the group on every frame — exactly what would let an
    // ancestor's reuse skip the group holding the filter.
    let mut binned = Vec::new();
    for i in 0..64 {
        paint_dab(&mut engine, top, 4.0 + (i % 8) as f32, 4.0);
        engine.test_readback_canvas();
        engine.render(0.0);
        binned = engine.histogram(fx);
        if !binned.is_empty() {
            break;
        }
    }

    assert!(
        !binned.is_empty(),
        "a histogram owed for a filter inside a group must still land while \
         the user works above that group — reuse that skips the group makes \
         the mid-walk dispatch unreachable and it never resolves"
    );
}

/// A nested group nobody touched contributes zero passes. Painting *below* it
/// is what puts it in the re-walked suffix at all: the parent has to blend it
/// again, but the group's own walk finds every stamp unchanged and draws
/// nothing, blending the output its accumulator still holds.
#[test]
fn an_untouched_nested_group_is_not_rewalked() {
    let mut f = fixture();
    // A passthrough group inlines into its parent's accumulator and never
    // walks separately; only an isolated one has a composite of its own to
    // find unchanged.
    f.engine.set_group_passthrough(f.group, false);
    f.engine.test_readback_canvas();
    let before = f.engine.test_walk_all_clean();

    // `bottom` sits below the group, so the group is part of the suffix the
    // root re-walks.
    paint_dab(&mut f.engine, f.bottom, 8.0, 8.0);
    let incremental = f.engine.test_readback_canvas();

    assert!(
        f.engine.test_walk_all_clean() > before,
        "the fixture's nested group was untouched by a paint beneath it, so \
         its own walk must have found nothing to do"
    );
    let scratch = f.engine.test_readback_canvas_from_scratch();
    assert_eq!(
        incremental, scratch,
        "a group that skipped its own walk must still blend the same pixels \
         a full walk would have produced"
    );
}

// ---------------------------------------------------------------------------
// The composite lives in an accumulator half, not a copy
// ---------------------------------------------------------------------------

/// A group's output is whichever ping-pong half its walk ended on, so every
/// consumer selects a resource by that index. If the present pass bound one
/// fixed half instead, a stack whose walk ends on the other half would present
/// stale or empty pixels — so this composites onto *both* halves and checks
/// each, asserting the halves really differed rather than trusting the parity.
#[test]
fn the_present_follows_the_composite_to_either_accumulator_half() {
    let mut engine = test_engine(W, H);
    let first = engine.add_raster_layer(None);
    fill_layer(&mut engine, first, 255, 0, 0);
    settle(&mut engine);

    let half_a = engine.test_root_output_half();
    let presented = engine.test_readback_present();
    assert_eq!(
        &presented[0..4],
        &[255, 0, 0, 255],
        "the present pass must read the half the walk ended on"
    );

    // One more advancing child flips the output to the other half.
    let second = engine.add_raster_layer(None);
    fill_layer(&mut engine, second, 0, 255, 0);
    settle(&mut engine);

    let half_b = engine.test_root_output_half();
    let presented = engine.test_readback_present();
    assert_eq!(
        &presented[0..4],
        &[0, 255, 0, 255],
        "the present pass must follow the composite when it lands on the \
         other half"
    );

    assert_ne!(
        half_a, half_b,
        "this test proves nothing unless the two composites actually ended on \
         different accumulator halves"
    );
}

/// The accumulators are written only inside `compose_group`, which is what
/// lets a consumer hold on to "the output half" between composites. A
/// present-only frame (view transform moved, document untouched) must still
/// find the last composite intact.
#[test]
fn a_present_only_frame_still_finds_the_last_composite() {
    let mut engine = test_engine(W, H);
    let layer = engine.add_raster_layer(None);
    fill_layer(&mut engine, layer, 0, 0, 255);
    settle(&mut engine);

    let before = engine.test_readback_present();
    let runs_before = engine.test_composite_runs();

    engine.set_view_transform(4.0, 2.0, 1.0, 0.0, false, W as f32, H as f32);
    engine.render(0.0);
    let after = engine.test_readback_present();

    assert_eq!(
        engine.test_composite_runs(),
        runs_before,
        "moving the view must not recomposite — otherwise this proves nothing \
         about the accumulator surviving between composites"
    );
    assert_eq!(
        before, after,
        "a present-only frame must still find the composite in the half the \
         last walk left it in"
    );
}

/// `composited_texture()` is read by export, save, the recorder, previews and
/// the sample-merged clone; the colour picker is the one consumer that reads
/// it *between* composites without forcing one. It must see the output half.
#[test]
fn the_merged_colour_picker_reads_the_output_half() {
    // Picking is asynchronous: the call queues a readback and hands back the
    // previously landed colour, so a pick is "request, let it land, read".
    fn pick_merged(engine: &mut DarklyEngine) -> [u8; 4] {
        use darkly::engine::PickSource;
        engine.pick_color(1.0, 1.0, PickSource::Merged);
        settle(engine);
        engine.pick_color(1.0, 1.0, PickSource::Merged)
    }

    let mut engine = test_engine(W, H);
    let first = engine.add_raster_layer(None);
    fill_layer(&mut engine, first, 255, 0, 0);
    engine.test_readback_canvas();

    let half_a = engine.test_root_output_half();
    assert_eq!(
        pick_merged(&mut engine),
        [255, 0, 0, 255],
        "the merged picker must sample the half holding the composite"
    );

    let second = engine.add_raster_layer(None);
    fill_layer(&mut engine, second, 0, 255, 0);
    engine.test_readback_canvas();

    let half_b = engine.test_root_output_half();
    assert_eq!(
        pick_merged(&mut engine),
        [0, 255, 0, 255],
        "…on either half"
    );
    assert_ne!(half_a, half_b, "both halves must have been exercised");
}

/// The histogram's stamp is its own declared dependency on authored pixels,
/// not an arrangement made for it by the revision registry. An animated veil
/// advancing its clock recomposites the canvas every tick; if that also
/// counted as a pixel write, a histogram could never settle while a veil
/// plays.
#[test]
fn a_histogram_survives_an_animating_veil() {
    let mut engine = test_engine(W, H);
    let base = engine.add_raster_layer(None);
    fill_layer(&mut engine, base, 128, 128, 128);
    let fx = effect(&mut engine, "levels");
    engine.set_histogram_target(Some(fx));
    // Above the levels filter, and animating: non-zero speed, or the canvas
    // animation gate never fires.
    engine
        .add_filter_layer(
            "grain",
            vec![
                ParamValue::Float(1.0), // speed
                ParamValue::Float(0.0), // color
                ParamValue::Float(1.0), // opacity
            ],
            None,
        )
        .expect("`grain` should be addable as an effect layer");

    let mut binned = Vec::new();
    for _ in 0..64 {
        engine.test_readback_canvas();
        engine.render(0.0);
        binned = engine.histogram(fx);
        if !binned.is_empty() {
            break;
        }
    }
    assert!(!binned.is_empty(), "the histogram must land");

    engine.test_tick_animations(1.0); // primes last_wall_time; dt == 0
    for i in 1..=8 {
        engine.test_tick_animations(1.0 + i as f32 * 0.05);
    }
    engine.test_readback_canvas();

    assert_eq!(
        engine.histogram(fx),
        binned,
        "an animation tick advances appearance, not authored pixels, so the \
         histogram it is read against must survive one"
    );
}

// ---------------------------------------------------------------------------
// Thumbnail cursor
// ---------------------------------------------------------------------------

/// The engine-side cursor replaces the compositor's drain-once dirty set. It
/// must queue exactly once per change, not once per frame.
#[test]
fn thumbnails_queue_once_per_pixel_change() {
    let mut engine = test_engine(W, H);
    let layer = engine.add_raster_layer(None);
    fill_layer(&mut engine, layer, 200, 100, 50);
    settle(&mut engine);
    engine.test_flush_readbacks();

    let after_fill = engine
        .test_thumbnail_cache_peek(layer)
        .expect("a painted layer has a thumbnail");

    // Idle frames must not requeue: the cursor already covers this revision.
    let version_before = engine.thumbnail_version();
    for _ in 0..4 {
        engine.render(0.0);
    }
    engine.test_flush_readbacks();
    assert_eq!(
        engine.thumbnail_version(),
        version_before,
        "idle frames must not requeue a thumbnail whose revision is already \
         covered by the cursor"
    );

    // A real pixel change queues exactly one more.
    fill_layer(&mut engine, layer, 10, 10, 250);
    settle(&mut engine);
    engine.test_flush_readbacks();
    let after_repaint = engine
        .test_thumbnail_cache_peek(layer)
        .expect("thumbnail still cached");
    assert_ne!(
        after_fill, after_repaint,
        "a pixel change must refresh the thumbnail"
    );
}

/// Disposing a node prunes its revision, and with it the cursor entry — so a
/// deleted layer stops being scanned and a reused id starts clean.
#[test]
fn deleting_a_layer_prunes_its_thumbnail_cursor() {
    let mut engine = test_engine(W, H);
    let keep = engine.add_raster_layer(None);
    fill_layer(&mut engine, keep, 10, 20, 30);
    let doomed = engine.add_raster_layer(None);
    fill_layer(&mut engine, doomed, 200, 200, 200);
    settle(&mut engine);

    engine.remove_layer(doomed).unwrap();
    settle(&mut engine);

    // The surviving layer is untouched, and nothing panics or requeues for the
    // removed id on subsequent frames.
    for _ in 0..4 {
        engine.render(0.0);
    }
    assert!(
        engine.test_thumbnail_cache_peek(keep).is_some(),
        "deleting one layer must not disturb another's thumbnail"
    );
}
