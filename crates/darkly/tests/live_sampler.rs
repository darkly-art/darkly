//! The live canvas sampler: `clone_source` on its Live source feeding
//! `paint`, which refreshes the stroke's appearance under each dab's read
//! region before that dab's dispatch (the Smudge brush).
//!
//! Runner-level harness: explicit dabs with explicit `motion`, one
//! `flush_dabs`, one `commit`, a readback of the layer. Each render takes the layer's plane rect and the canvas
//! origin, so the frame-sensitive tests run once at the origin and once on
//! an offset layer under a cropped canvas.

use std::sync::{Arc, OnceLock};

use darkly::brush::compile_graph;
use darkly::brush::gpu_context::{
    BrushGpuContext, BrushPerfCounters, CursorPreviewState, DabBatch, StrokeResources,
};
use darkly::brush::input_value::InputValue;
use darkly::brush::paint_info::PaintInformation;
use darkly::brush::pipeline::BrushPipelines;
use darkly::brush::stroke_buffer::StrokeBuffer;
use darkly::brush::texture_source::{LiveSource, ResolvedSource};
use darkly::brush::wire::BrushWireType;
use darkly::coord::CanvasRect;
use darkly::gpu::test_utils::{create_test_texture, readback_texture, test_device};
use darkly::nodegraph::{Graph, NodeId, PortRef};

const SIDE: u32 = 128;

fn shared_device() -> (Arc<wgpu::Device>, Arc<wgpu::Queue>) {
    static HANDLES: OnceLock<(Arc<wgpu::Device>, Arc<wgpu::Queue>)> = OnceLock::new();
    HANDLES
        .get_or_init(|| {
            let (d, q) = test_device();
            (Arc::new(d), Arc::new(q))
        })
        .clone()
}

// ── Frames ──────────────────────────────────────────────────────────────

/// Where the painted layer sits on the plane and where the canvas window
/// is anchored. Positions in a test are layer-local and go through
/// [`Frame::plane`], so one test body runs in both frames.
#[derive(Clone, Copy, Debug)]
struct Frame {
    layer_offset: [i32; 2],
    canvas_origin: [i32; 2],
}

const ORIGIN: Frame = Frame {
    layer_offset: [0, 0],
    canvas_origin: [0, 0],
};

/// An offset layer under a cropped canvas: neither the layer nor the
/// window sits at the plane origin.
const OFFSET: Frame = Frame {
    layer_offset: [-37, 21],
    canvas_origin: [-50, 9],
};

const FRAMES: [Frame; 2] = [ORIGIN, OFFSET];

impl Frame {
    fn plane(&self, local: [f32; 2]) -> [f32; 2] {
        [
            local[0] + self.layer_offset[0] as f32,
            local[1] + self.layer_offset[1] as f32,
        ]
    }
}

// ── Canvases ────────────────────────────────────────────────────────────

fn canvas(f: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
    let mut out = Vec::with_capacity((SIDE * SIDE * 4) as usize);
    for y in 0..SIDE {
        for x in 0..SIDE {
            out.extend_from_slice(&f(x, y));
        }
    }
    out
}

fn pixel(rgba: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * SIDE + x) * 4) as usize;
    [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
}

/// Opaque: red to the left of `x = 36`, black elsewhere.
fn two_tone() -> Vec<u8> {
    canvas(|x, _| {
        if x < 36 {
            [220, 20, 20, 255]
        } else {
            [0, 0, 0, 255]
        }
    })
}

/// Opaque horizontal ramp, two levels per pixel, so a shift of `k` pixels
/// changes red by `2k` exactly.
fn ramp() -> Vec<u8> {
    canvas(|x, y| [(2 * x) as u8, (y % 7 * 30) as u8, 90, 255])
}

// ── Graphs ──────────────────────────────────────────────────────────────

fn id(s: &str) -> NodeId {
    NodeId(s.into())
}

fn port(node: &str, port: &str) -> PortRef {
    PortRef {
        node: id(node),
        port: port.into(),
    }
}

fn builtin(name: &str) -> Graph<BrushWireType> {
    darkly::brush::builtin_brushes::all()
        .into_iter()
        .find(|b| b.metadata.name == name)
        .unwrap_or_else(|| panic!("built-in brush `{name}`"))
        .metadata
        .graph
}

fn set(graph: &mut Graph<BrushWireType>, node: &str, name: &str, value: f32) {
    graph.set_port_default(&id(node), name, value).unwrap();
}

/// The shipped Smudge at a radius of 20 px under full pressure. `None`
/// keeps the brush's move law; `Some(buildup)` switches `paint` to the
/// deposit dial at that setting, to pin the laws on either side of it.
fn smooth_smudge(softness: f32, buildup: Option<f32>, strength: f32) -> Graph<BrushWireType> {
    let mut g = builtin("Smudge");
    set(&mut g, "circle", "softness", softness);
    if let Some(buildup) = buildup {
        g.set_port_value(&id("paint"), "mode", InputValue::Int(0))
            .unwrap();
        set(&mut g, "paint", "buildup", buildup);
    }
    set(&mut g, "user_input", "value", strength);
    set(&mut g, "brush_settings", "size", RADIUS * 2.0 / 512.0);
    g
}

const RADIUS: f32 = 20.0;

// ── Harness ─────────────────────────────────────────────────────────────

struct Run {
    /// The committed layer, layer-local.
    layer: Vec<u8>,
    perf: BrushPerfCounters,
}

/// One `(layer-local position, motion)` per dab, all in one flush, then the
/// commit.
fn render(
    graph: &Graph<BrushWireType>,
    frame: Frame,
    pre: &[u8],
    dabs: &[([f32; 2], [f32; 2])],
) -> Run {
    let (device, queue) = shared_device();
    let extent = CanvasRect::from_xywh(frame.layer_offset[0], frame.layer_offset[1], SIDE, SIDE);
    let (layer_texture, layer_view) = create_test_texture(&device, &queue, SIDE, SIDE, pre);
    let pipelines = BrushPipelines::new(
        &device,
        &queue,
        &darkly::gpu::selection::selection_mask_bgl(&device),
    );
    let mut runner = compile_graph(graph).expect("brush compiles");
    let mut stroke_buffer = StrokeBuffer::new(
        &device,
        SIDE,
        SIDE,
        &pipelines,
        runner.scratch_format(),
        runner.dab_pass(),
    );
    let target = || {
        darkly::gpu::paint_target::GpuPaintTarget::from_canvas_texture(
            &layer_texture,
            &layer_view,
            wgpu::TextureFormat::Rgba8Unorm,
            extent,
        )
    };
    let mut enc = device.create_command_encoder(&Default::default());
    stroke_buffer.save_pre_stroke(&device, &mut enc, &pipelines, &target());
    queue.submit([enc.finish()]);

    macro_rules! make_ctx {
        () => {{
            let (scratch, pre_stroke_tex, pre_stroke_bg, source_override) =
                stroke_buffer.parts_for_brush_ctx();
            BrushGpuContext {
                encoder: device.create_command_encoder(&Default::default()),
                device: &device,
                queue: &queue,
                pipelines: &pipelines,
                selection_bind_group: pipelines.default_selection_bind_group(),
                canvas_width: SIDE,
                canvas_height: SIDE,
                canvas_origin: frame.canvas_origin,
                blend_mode: 0,
                view_rotation: 0.0,
                dpi_factor: 1.0,
                perf: BrushPerfCounters::default(),
                stroke: Some(StrokeResources {
                    scratch,
                    paint_target: target(),
                    pre_stroke_texture: pre_stroke_tex,
                    pre_stroke_bind_group: pre_stroke_bg,
                    source_override,
                }),
                preview: None,
                dab_batch: DabBatch::default(),
            }
        }};
    }

    {
        let mut ctx = make_ctx!();
        runner.begin_stroke(&mut ctx, None);
        queue.submit([ctx.encoder.finish()]);
    }
    let perf = {
        let mut ctx = make_ctx!();
        for (i, (pos, motion)) in dabs.iter().enumerate() {
            let info = PaintInformation {
                pos: frame.plane(*pos),
                motion: *motion,
                pressure: 1.0,
                distance: 10.0,
                ..Default::default()
            };
            runner.clear_slots();
            runner.seed_sensors(&info, [1.0; 4], 0xC0FFEE, i as u32);
            runner.execute_cpu();
            runner.execute_gpu(&mut ctx);
        }
        runner.flush_dabs(&mut ctx);
        runner.commit(&mut ctx);
        ctx.submit_final()
    };
    Run {
        layer: readback_texture(
            &device,
            &queue,
            &layer_texture,
            wgpu::TextureFormat::Rgba8Unorm,
            SIDE,
            SIDE,
        ),
        perf,
    }
}

fn dist(a: [f32; 2], b: [f32; 2]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// Layer-local pixel centre of `(x, y)`.
fn centre(x: u32, y: u32) -> [f32; 2] {
    [x as f32 + 0.5, y as f32 + 0.5]
}

// ── Compile ─────────────────────────────────────────────────────────────

/// The Smudge plus a `noise` grain and an `image` on the tip compiles with
/// a baked, a named and a live slot, reads the stroke appearance, and
/// samples it only in the stroke module; the clone brush and a plain disc
/// do not read it; and an instanced terminal cannot host it.
#[test]
fn live_sampler_compiles_beside_baked_noise_and_image() {
    let reg = darkly::brush::registry();
    let mut g = builtin("Smudge");
    let noise = g.add_node("noise", reg.get("noise").unwrap().ports.clone());
    let grain = g.add_node("multiply", reg.get("multiply").unwrap().ports.clone());
    let image = g.add_node("image", reg.get("image").unwrap().ports.clone());
    g.set_port_value(&image, "texture_name", InputValue::String("paper".into()))
        .unwrap();
    let split = g.add_node("split_color", reg.get("split_color").unwrap().ports.clone());
    let paper = g.add_node("multiply", reg.get("multiply").unwrap().ports.clone());
    let at = |node: &NodeId, port: &str| PortRef {
        node: node.clone(),
        port: port.into(),
    };
    g.disconnect(&port("circle", "mask"), &port("stamp", "tip"));
    g.connect(port("circle", "mask"), at(&grain, "a")).unwrap();
    g.connect(at(&noise, "value"), at(&grain, "b")).unwrap();
    g.connect(at(&grain, "result"), at(&paper, "a")).unwrap();
    g.connect(at(&image, "color"), at(&split, "color")).unwrap();
    g.connect(at(&split, "luminance"), at(&paper, "b")).unwrap();
    g.connect(at(&paper, "result"), port("stamp", "tip"))
        .unwrap();
    let runner = compile_graph(&g).expect("compiles");
    let compiled = runner
        .compiled_brush()
        .expect("a paint graph compiles to WGSL");
    let sources = &compiled.graph_sources;
    assert!(sources
        .iter()
        .any(|s| matches!(s, ResolvedSource::Baked(_))));
    assert!(sources
        .iter()
        .any(|s| matches!(s, ResolvedSource::Named(_))));
    let live = sources
        .iter()
        .position(|s| *s == ResolvedSource::Live(LiveSource::StrokeAppearance))
        .expect("the live slot");
    assert!(compiled.reads_stroke_appearance());
    assert!(compiled
        .stroke_wgsl
        .contains(&format!("textureLoad(graph_tex_{live}")));
    assert!(
        !compiled
            .cursor_preview_wgsl
            .contains("= clone_live_clone_source("),
        "the preview body never calls the live helper"
    );
    assert!(compiled.stroke_wgsl.contains("= clone_live_clone_source("));

    for name in ["Clone", "Ink Pen"] {
        let runner = compile_graph(&builtin(name)).unwrap();
        assert!(
            !runner.compiled_brush().unwrap().reads_stroke_appearance(),
            "{name}"
        );
    }

    // Re-terminated in an instanced terminal that takes a colour.
    let mut g = builtin("Smudge");
    g.remove_node(&id("paint")).unwrap();
    let wc = g.add_node("watercolor", reg.get("watercolor").unwrap().ports.clone());
    let at = |port: &str| PortRef {
        node: wc.clone(),
        port: port.into(),
    };
    g.connect(port("pen_input", "position"), at("position"))
        .unwrap();
    g.connect(port("clone_source", "color"), at("color"))
        .unwrap();
    let Err(err) = compile_graph(&g) else {
        panic!("an instanced pass cannot refresh between dabs");
    };
    assert!(err.contains("live stroke appearance"), "{err}");
    assert!(err.contains("instanced"), "{err}");
}

// ── The snapshot's ordering ─────────────────────────────────────────────

/// A hard disc at full build-up and full flow replaces the ground with
/// what it samples, so inside the disc the layer is the ramp shifted by
/// exactly the motion. A second dab whose samples land in the first dab's
/// interior shows the shift twice. Exact only if every read saw the stroke
/// as it stood before the reading dab: a feature test of the snapshot's
/// ordering. Interior pixels only: the disc's one-texel antialiased rim is
/// a partial deposit.
#[test]
fn sampler_reads_through_the_snapshot_not_the_racing_ground() {
    let pre = ramp();
    let graph = smooth_smudge(0.0, None, 1.0);
    let (c1, c2, m) = ([64.0, 64.0], [70.0, 64.0], [3.0, 0.0]);
    for frame in FRAMES {
        let one = render(&graph, frame, &pre, &[(c1, m)]);
        let two = render(&graph, frame, &pre, &[(c1, m), (c2, m)]);
        let (mut checked_one, mut checked_two) = (0, 0);
        for y in 0..SIDE {
            for x in 0..SIDE {
                let p = centre(x, y);
                let src = [p[0] - m[0], p[1] - m[1]];
                if dist(p, c1) < RADIUS - 1.5 {
                    assert_eq!(
                        pixel(&one.layer, x, y),
                        pixel(&pre, x - 3, y),
                        "{frame:?}: one dab at ({x}, {y}) is the ramp shifted by 3"
                    );
                    checked_one += 1;
                }
                if dist(p, c2) < RADIUS - 1.5 {
                    let want = if dist(src, c1) < RADIUS - 1.5 {
                        pixel(&pre, x - 6, y)
                    } else if dist(src, c1) > RADIUS + 1.5 {
                        pixel(&pre, x - 3, y)
                    } else {
                        continue;
                    };
                    assert_eq!(
                        pixel(&two.layer, x, y),
                        want,
                        "{frame:?}: the second dab at ({x}, {y}) reads the first's deposit"
                    );
                    checked_two += 1;
                }
            }
        }
        assert!(
            checked_one > 900 && checked_two > 900,
            "{checked_one} {checked_two}"
        );
    }
}

/// Dab 2's sample point lies inside dab 1's footprint, so the red dab 1
/// pulled out of the bar reaches dab 2; a control render of dab 2 alone
/// reads the black pre-stroke there.
#[test]
fn second_dab_reads_first_dabs_deposit() {
    let pre = two_tone();
    let graph = smooth_smudge(0.4, None, 0.85);
    let dab1 = ([60.0, 64.0], [30.0, 0.0]);
    let dab2 = ([90.0, 64.0], [30.0, 0.0]);
    let both = render(&graph, ORIGIN, &pre, &[dab1, dab2]);
    let alone = render(&graph, ORIGIN, &pre, &[dab2]);
    let with = pixel(&both.layer, 90, 64);
    let without = pixel(&alone.layer, 90, 64);
    assert!(with[0] > 80 && with[0] > with[1] + 30, "{with:?}");
    assert!(without[0] < 10, "{without:?}");
}

// ── The laws ────────────────────────────────────────────────────────────

/// The wash refuses a repeated smear at the same pressure like the Pencil
/// refuses a retraced line; full build-up takes it. Two dabs at one
/// position with one motion, longer than the footprint so both sample the
/// untouched bar and carry the same black at the same coverage.
#[test]
fn wash_refuses_a_repeated_smear_like_the_pencil() {
    let pre = canvas(|x, _| {
        if (10..40).contains(&x) {
            [0, 0, 0, 255]
        } else {
            [255, 255, 255, 255]
        }
    });
    let dab = ([90.0, 64.0], [60.0, 0.0]);
    let darkness = |rgba: &[u8]| -> u64 {
        (0..SIDE * SIDE)
            .map(|i| 255 - rgba[(i * 4) as usize] as u64)
            .sum()
    };

    let wash = smooth_smudge(0.4, Some(0.0), 0.6);
    let once = render(&wash, ORIGIN, &pre, &[dab]);
    let twice = render(&wash, ORIGIN, &pre, &[dab, dab]);
    assert!(
        darkness(&once.layer) > darkness(&pre),
        "the first pass smears"
    );
    assert_eq!(
        once.layer, twice.layer,
        "the wash takes a repeated smear once"
    );

    let build = smooth_smudge(0.4, Some(1.0), 0.6);
    let once = render(&build, ORIGIN, &pre, &[dab]);
    let twice = render(&build, ORIGIN, &pre, &[dab, dab]);
    assert!(
        darkness(&twice.layer) > darkness(&once.layer),
        "full build-up compounds the repeat"
    );
}

/// A dab that has not moved deposits nothing: depositing the appearance
/// over itself raises alpha on a partially transparent pixel, a dot at
/// every stroke start. The mark is authored with `rgb = 0` under `a = 0`;
/// alpha is compared exactly and rgb where there is coverage, since the
/// commit zeroes rgb under transparent texels.
#[test]
fn stationary_dab_deposits_nothing() {
    let pre = canvas(|x, y| {
        let d = dist(centre(x, y), [64.0, 64.0]);
        let a = (255.0 * (1.0 - d / 30.0)).clamp(0.0, 255.0) as u8;
        if a == 0 {
            [0, 0, 0, 0]
        } else {
            [200, 60, 30, a]
        }
    });
    let run = render(
        &smooth_smudge(0.4, None, 1.0),
        ORIGIN,
        &pre,
        &[([64.0, 64.0], [0.0, 0.0])],
    );
    for y in 0..SIDE {
        for x in 0..SIDE {
            let (got, want) = (pixel(&run.layer, x, y), pixel(&pre, x, y));
            assert_eq!(got[3], want[3], "alpha at ({x}, {y})");
            if want[3] > 0 {
                assert_eq!(got, want, "colour at ({x}, {y})");
            }
        }
    }
}

/// The opacity port is a commit-time cap, as on every `paint` brush: the
/// stroke at half opacity is the full-opacity stroke mixed halfway back to
/// the layer it started from.
#[test]
fn opacity_is_a_commit_time_cap() {
    let pre = two_tone();
    let dabs = [
        ([44.0, 64.0], [6.0, 0.0]),
        ([50.0, 64.0], [6.0, 0.0]),
        ([56.0, 64.0], [6.0, 0.0]),
    ];
    let full = render(&smooth_smudge(0.4, None, 0.8), ORIGIN, &pre, &dabs);
    let mut graph = smooth_smudge(0.4, None, 0.8);
    set(&mut graph, "paint", "opacity", 0.5);
    let half = render(&graph, ORIGIN, &pre, &dabs);
    let mut moved = 0;
    for (i, ((h, f), p)) in half.layer.iter().zip(&full.layer).zip(&pre).enumerate() {
        let want = (*p as f32 + *f as f32) * 0.5;
        assert!(
            (*h as f32 - want).abs() <= 1.0,
            "byte {i}: half {h}, full {f}, pre {p}"
        );
        moved += usize::from(f != p);
    }
    assert!(moved > 100, "the full stroke moved pigment");
}

// ── Frames and borders ──────────────────────────────────────────────────

/// At the layer border the bilinear filter must not wrap to the opposite
/// edge: the read region is clamped to the layer, so the opposite edge of
/// the mirror was never refreshed for this dab. A dab whose fractional
/// motion lands a sample within half a texel of the right edge (pixel 124
/// samples at 127.75), over a uniform grey with a saturated left edge,
/// must leave the grey exactly as it was.
#[test]
fn border_dab_never_reads_the_opposite_edge() {
    let pre = canvas(|x, _| {
        if x < 8 {
            [255, 0, 0, 255]
        } else {
            [128, 128, 128, 255]
        }
    });
    let graph = smooth_smudge(0.0, None, 1.0);
    for frame in FRAMES {
        let run = render(&graph, frame, &pre, &[([118.0, 64.0], [-3.25, 0.0])]);
        for y in 0..SIDE {
            for x in 64..SIDE {
                assert_eq!(
                    pixel(&run.layer, x, y),
                    [128, 128, 128, 255],
                    "{frame:?}: ({x}, {y}) read past the border"
                );
            }
        }
    }
}

// ── Dispatch count ──────────────────────────────────────────────────────

/// Every dab of a live-sampling brush is two dispatches: the snapshot and
/// the dab.
#[test]
fn live_sampling_dispatches_twice_per_dab() {
    let dabs = [([40.0, 64.0], [4.0, 0.0]), ([44.0, 64.0], [4.0, 0.0])];
    let run = render(
        &smooth_smudge(0.4, Some(0.1), 0.6),
        ORIGIN,
        &two_tone(),
        &dabs,
    );
    assert_eq!(run.perf.flushed_dabs, 2);
    assert_eq!(run.perf.dispatches, 4);
}

// ── Hover ───────────────────────────────────────────────────────────────

/// At hover there is no stroke to sample: the preview shows the tip in
/// the sampler's neutral grey, never NaN.
#[test]
fn hover_preview_is_a_neutral_footprint() {
    const PREVIEW: u32 = 256;
    let (device, queue) = shared_device();
    let pipelines = BrushPipelines::new(
        &device,
        &queue,
        &darkly::gpu::selection::selection_mask_bgl(&device),
    );
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("preview-target"),
        size: wgpu::Extent3d {
            width: PREVIEW,
            height: PREVIEW,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let mut runner = compile_graph(&builtin("Smudge")).expect("compiles");
    let mut ctx = BrushGpuContext {
        encoder: device.create_command_encoder(&Default::default()),
        device: &device,
        queue: &queue,
        pipelines: &pipelines,
        selection_bind_group: pipelines.default_selection_bind_group(),
        canvas_width: PREVIEW,
        canvas_height: PREVIEW,
        canvas_origin: [0, 0],
        blend_mode: 0,
        view_rotation: 0.0,
        dpi_factor: 1.0,
        perf: BrushPerfCounters::default(),
        stroke: None,
        preview: Some(CursorPreviewState {
            mask_view: Some(&view),
            mask_size: (PREVIEW, PREVIEW),
            mask_overlay: None,
            info: None,
        }),
        dab_batch: DabBatch::default(),
    };
    let info = PaintInformation {
        pos: [PREVIEW as f32 * 0.5; 2],
        pressure: 1.0,
        ..Default::default()
    };
    runner.seed_sensors(&info, [1.0; 4], 0xC0FFEE, 0);
    runner.execute_cpu();
    runner.render_cursor_preview_pipeline(&mut ctx);
    assert!(ctx.preview.as_ref().and_then(|p| p.info).is_some());
    queue.submit([ctx.encoder.finish()]);
    let rgba = readback_texture(
        &device,
        &queue,
        &target,
        wgpu::TextureFormat::Rgba8Unorm,
        PREVIEW,
        PREVIEW,
    );
    let i = ((PREVIEW / 2 * PREVIEW + PREVIEW / 2) * 4) as usize;
    let c = &rgba[i..i + 4];
    assert!(c[3] > 0, "the tip has coverage at its centre: {c:?}");
    assert!(
        (c[0] as i32 - c[1] as i32).abs() < 5 && (c[1] as i32 - c[2] as i32).abs() < 5,
        "neutral grey: {c:?}"
    );
}

// ── Transparency ────────────────────────────────────────────────────────

/// Smearing a uniform half-transparent field along itself is the identity:
/// every sample equals the pixel it lands on, so a finger that only moves
/// pigment leaves the field as it was. A law that lays the sample *over*
/// the pixel instead raises alpha wherever it is partial, which reads as a
/// darkening of every soft edge on a transparent layer (99 levels here).
///
/// Tolerance: the moved pigment and the coverage are each stored at 8 bits
/// per dab, so three dabs can drift by a couple of levels.
#[test]
fn smearing_a_translucent_field_along_itself_is_the_identity() {
    let pre = canvas(|x, _| {
        if (20..110).contains(&x) {
            [40, 40, 40, 128]
        } else {
            [0, 0, 0, 0]
        }
    });
    let graph = smooth_smudge(0.4, None, 0.6);
    let dabs = [
        ([64.0, 50.0], [0.0, 6.0]),
        ([64.0, 56.0], [0.0, 6.0]),
        ([64.0, 62.0], [0.0, 6.0]),
    ];
    let run = render(&graph, ORIGIN, &pre, &dabs);
    let mut worst = 0;
    for y in 0..SIDE {
        for x in 30..100 {
            let (got, want) = (pixel(&run.layer, x, y), pixel(&pre, x, y));
            for c in 0..4 {
                worst = worst.max((got[c] as i32 - want[c] as i32).abs());
            }
        }
    }
    assert!(worst <= 2, "the field changed by up to {worst} levels");
}

/// Dragging bare paper onto a translucent edge thins it, and dragging the
/// edge onto bare paper thickens the paper: the finger moves pigment, so
/// alpha falls behind the stroke and rises ahead of it.
#[test]
fn a_smear_thins_the_edge_it_leaves_and_thickens_where_it_lands() {
    let pre = canvas(|x, _| {
        if x < 64 {
            [40, 40, 40, 128]
        } else {
            [0, 0, 0, 0]
        }
    });
    // A hard disc at full flow, moving right by 8 px: every pixel takes
    // what was 8 px to its left.
    let run = render(
        &smooth_smudge(0.0, None, 1.0),
        ORIGIN,
        &pre,
        &[([64.0, 64.0], [8.0, 0.0])],
    );
    let ahead = pixel(&run.layer, 68, 64);
    let behind = pixel(&run.layer, 60, 64);
    assert_eq!(ahead, [40, 40, 40, 128], "pigment landed on bare paper");
    assert_eq!(behind[3], 128, "the interior is unchanged");
    // At the trailing edge of the layer's own bar nothing came from the
    // left of pixel 8: the dab did not reach it. Check instead that a dab
    // whose samples come from bare paper erases the bar.
    let run = render(
        &smooth_smudge(0.0, None, 1.0),
        ORIGIN,
        &pre,
        &[([60.0, 64.0], [-8.0, 0.0])],
    );
    let thinned = pixel(&run.layer, 60, 64);
    assert_eq!(
        thinned[3], 0,
        "bare paper dragged onto the edge thinned it: {thinned:?}"
    );
}

/// The sampler interpolates in premultiplied space: a sample point halfway
/// between an opaque red texel and a transparent one is red at half alpha,
/// not dark red (lesson 2 of `docs/lessons-learned/compositing-lessons-learned.md`).
#[test]
fn sampler_interpolates_premultiplied_across_a_transparent_edge() {
    let pre = canvas(|x, _| {
        if x < 64 {
            [255, 0, 0, 255]
        } else {
            [0, 0, 0, 0]
        }
    });
    // A hard disc at full flow replaces the field with the sample; motion
    // of 3.5 px puts every sample point halfway between texels.
    let graph = smooth_smudge(0.0, None, 1.0);
    let run = render(&graph, ORIGIN, &pre, &[([70.0, 64.0], [3.5, 0.0])]);
    // Pixel 67 samples at 63.5: half red, half transparent.
    let got = pixel(&run.layer, 67, 64);
    assert!(got[3] > 100 && got[3] < 156, "{got:?}");
    assert!(
        got[0] > 240,
        "red must stay red at a transparent edge: {got:?}"
    );
}

/// Smearing a uniform dark opaque surface along itself is the identity to
/// the level, at the soft edge too. A soft tip brings a few percent of
/// coverage per dab there; under premultiplied 8-bit storage a dark colour
/// times that coverage rounds to zero while the alpha does not, so the
/// pigment the finger carries turns black and the stroke's edge darkens.
#[test]
fn a_soft_smear_over_a_dark_surface_is_the_identity() {
    let pre = canvas(|_, _| [17, 22, 28, 255]);
    let graph = smooth_smudge(1.0, None, 0.5);
    let dabs: Vec<([f32; 2], [f32; 2])> = (0..40)
        .map(|i| ([30.0 + i as f32, 64.0], [1.0, 0.0]))
        .collect();
    let run = render(&graph, ORIGIN, &pre, &dabs);
    let mut worst = 0;
    for y in 0..SIDE {
        for x in 0..SIDE {
            let (got, want) = (pixel(&run.layer, x, y), pixel(&pre, x, y));
            for c in 0..4 {
                worst = worst.max((got[c] as i32 - want[c] as i32).abs());
            }
        }
    }
    assert!(worst <= 1, "the surface changed by up to {worst} levels");
}
