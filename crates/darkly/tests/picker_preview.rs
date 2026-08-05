//! Picker preview generation, over every previewable catalog.
//!
//! The engine half: that a request enqueues, that generation is paced across
//! ticks, that the frames move, that polling hands the job over, and that none
//! of it touches the document. Plus the property the whole design rests on —
//! `preview_at` is absolute, so a sequence reaches the same state at `t`
//! however it got there.
//!
//! Uses the blocking readback flush (`test_flush_readbacks`) — native-only; the
//! wasm path drains the same `ReadbackScheduler` from the rAF render loop.
//!
//! Run with: `cargo test -p darkly --features testing --test picker_preview -- --test-threads=1`

use darkly::catalog::preview_mechanisms;
use darkly::engine::preview::PREVIEW_FRAMES_PER_TICK;
use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::preview::{
    drive, fit_preview_dims, PreviewRegistries, PreviewSequence, PreviewTarget, PREVIEW_FORMAT,
};
use darkly::gpu::test_utils::{readback_texture, test_device};

/// A `w × h` RGBA gradient. Deliberately not a flat fill: an effect that
/// redistributes colour — a refraction, a blur, a pixelation — leaves a solid
/// canvas exactly as it found it, so a flat subject would make every motion
/// assertion below vacuous.
fn gradient(w: u32, h: u32) -> Vec<u8> {
    let mut pixels = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            pixels[i..i + 4].copy_from_slice(&[
                (x * 255 / w.max(1)) as u8,
                (y * 255 / h.max(1)) as u8,
                ((x + y) * 127 / (w + h).max(1)) as u8,
                255,
            ]);
        }
    }
    pixels
}

/// Headless engine whose canvas holds real content, so the composite a
/// source-reading preview samples has something to act on.
fn headless_engine(w: u32, h: u32) -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    let mut engine = DarklyEngine::new(gpu, w, h);
    engine.paste_image(w, h, &gradient(w, h), 0, 0, None);
    engine
}

/// Run the engine's own frame loop until the preview completes, or give up
/// after a generous bound. Each tick pumps at most `PREVIEW_FRAMES_PER_TICK`
/// frames and drains whatever landed, which is exactly what the browser does.
/// Returns `(width, height, fps, frames)`.
fn drain_preview(
    engine: &mut DarklyEngine,
    catalog: &str,
    type_id: &str,
) -> (u32, u32, u32, Vec<Vec<u8>>) {
    for _ in 0..512 {
        if let Some(result) = engine.poll_preview(catalog, type_id) {
            return result;
        }
        engine.render(0.0);
        engine.test_flush_readbacks();
    }
    panic!("preview for {catalog}/{type_id} never completed");
}

/// One preview target loaded with a flat source, plus the registries a session
/// opens against — the pieces both the engine and the documentation binary
/// assemble, here without either.
struct Offscreen {
    gpu: (wgpu::Device, wgpu::Queue),
    target: PreviewTarget,
    veils: darkly::gpu::veil::VeilRegistry,
    voids: darkly::gpu::void::VoidRegistry,
    filters: darkly::gpu::filter::FilterPipelineRegistry,
    _source: wgpu::Texture,
}

impl Offscreen {
    fn new() -> Self {
        const DIM: u32 = 64;
        let (device, queue) = test_device();
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("picker-preview-test-source"),
            size: wgpu::Extent3d {
                width: DIM,
                height: DIM,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: PREVIEW_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            texture.as_image_copy(),
            &gradient(DIM, DIM),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(DIM * 4),
                rows_per_image: Some(DIM),
            },
            wgpu::Extent3d {
                width: DIM,
                height: DIM,
                depth_or_array_layers: 1,
            },
        );

        let mut target = PreviewTarget::new();
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        target.load_source(&device, &queue, &view, DIM, DIM);

        Offscreen {
            gpu: (device, queue),
            target,
            veils: darkly::gpu::veil::VeilRegistry::new(),
            voids: darkly::gpu::void::VoidRegistry::new(),
            filters: darkly::gpu::filter::FilterPipelineRegistry::new(),
            _source: texture,
        }
    }

    /// Every frame of one entry, driven through the blocking sink.
    fn render(&mut self, catalog: &str, type_id: &str) -> Vec<Vec<u8>> {
        self.render_range(catalog, type_id, 0)
    }

    /// Frames from `start` onward, so a test can compare a sequence resumed
    /// mid-run against one run from the beginning.
    fn render_range(&mut self, catalog: &str, type_id: &str, start: u32) -> Vec<Vec<u8>> {
        let (_, mech) = preview_mechanisms()
            .into_iter()
            .find(|(id, _)| *id == catalog)
            .unwrap_or_else(|| panic!("`{catalog}` has a preview mechanism"));
        if mech.reads_source() {
            // Already loaded in `new`.
        } else {
            let (device, queue) = &self.gpu;
            self.target.clear_source(device, queue, 64, 64);
        }
        let (w, h) = self.target.size();
        let (device, queue) = (self.gpu.0.clone(), self.gpu.1.clone());
        let Offscreen {
            target,
            veils,
            voids,
            filters,
            ..
        } = self;
        let regs = PreviewRegistries {
            veils,
            voids,
            filters,
        };
        let mut seq = PreviewSequence::open(mech, regs, type_id)
            .unwrap_or_else(|| panic!("`{catalog}/{type_id}` opens"));
        seq.seek(start);
        let mut frames = Vec::new();
        drive(
            &mut seq,
            &device,
            &queue,
            target,
            |encoder, output, _, _| {
                queue.submit([encoder.finish()]);
                frames.push(readback_texture(
                    &device,
                    &queue,
                    output,
                    PREVIEW_FORMAT,
                    w,
                    h,
                ));
            },
        );
        frames
    }
}

/// Every previewable entry of every offscreen catalog, as `(catalog, type_id)`.
/// Reads the generated mechanism table, so a new previewable catalog is covered
/// by every test below without one of them being edited.
fn offscreen_entries() -> Vec<(&'static str, &'static str)> {
    let mut out = Vec::new();
    for cat in darkly::catalog::catalogs() {
        let Some((id, mech)) = preview_mechanisms()
            .into_iter()
            .find(|(id, _)| *id == cat.id)
        else {
            continue;
        };
        for e in cat.entries.iter().filter(|e| e.supports_preview) {
            let entry = mech
                .resolve(e.type_id)
                .unwrap_or_else(|| panic!("`{id}/{}` resolves", e.type_id));
            out.push((id, entry.type_id));
        }
    }
    assert!(!out.is_empty(), "no offscreen previewable entries at all");
    out
}

// ---------------------------------------------------------------------------
// The property the design rests on
// ---------------------------------------------------------------------------

/// A sequence resumed mid-run renders exactly what an uninterrupted one would.
///
/// This is what replaces the whole rebuild-and-replay apparatus a delta-driven
/// preview needs: because `preview_at(t)` puts an instance into a state that
/// depends on `t` and nothing else, the engine can drop a sequence at the end of
/// a tick and re-open it on the next one without replaying a single frame. Every
/// previewable entry is held to it — including the two that rebuild their cache
/// mid-sweep, where the property is least obvious.
#[test]
fn preview_at_is_absolute() {
    let mut off = Offscreen::new();
    for (catalog, type_id) in offscreen_entries() {
        let whole = off.render(catalog, type_id);
        if whole.len() < 4 {
            continue;
        }
        let start = (whole.len() / 2) as u32;
        let resumed = off.render_range(catalog, type_id, start);
        assert_eq!(
            resumed.len(),
            whole.len() - start as usize,
            "`{catalog}/{type_id}` resumed with the wrong frame count"
        );
        for (i, frame) in resumed.iter().enumerate() {
            assert_eq!(
                frame,
                &whole[start as usize + i],
                "`{catalog}/{type_id}` frame {} differs when the sequence is resumed \
                 rather than run from the start",
                start as usize + i
            );
        }
    }
}

/// Two runs of the same entry through the same target produce the same pixels.
///
/// Determinism is what lets the engine hand a half-finished job across ticks and
/// what lets the documentation renderer reuse one device for thirty-four assets.
/// A session that left state behind — in its instance, its cache, or the shared
/// target — shows up here.
#[test]
fn rendering_an_entry_twice_produces_the_same_frames() {
    let mut off = Offscreen::new();
    for (catalog, type_id) in offscreen_entries() {
        let first = off.render(catalog, type_id);
        let again = off.render(catalog, type_id);
        assert_eq!(
            first, again,
            "`{catalog}/{type_id}` rendered two different sequences"
        );
    }
}

/// Every entry declaring more than one frame renders at least two distinct
/// images, and an entry declaring one renders exactly one.
///
/// The defect this whole change exists to fix: `frozen` and `pixelate` shipped
/// as single stills at their schema defaults because the live path never read
/// their declared motion.
#[test]
fn every_animated_entry_actually_moves() {
    let mut off = Offscreen::new();
    for (catalog, type_id) in offscreen_entries() {
        let frames = off.render(catalog, type_id);
        assert!(!frames.is_empty(), "`{catalog}/{type_id}` rendered nothing");
        if frames.len() == 1 {
            continue;
        }
        assert!(
            frames.windows(2).any(|pair| pair[0] != pair[1]),
            "`{catalog}/{type_id}` rendered {} identical frames",
            frames.len()
        );
    }
}

/// One sequence over one target, run through the blocking sink and through a
/// recording stand-in for the engine's asynchronous one, produces the same
/// frames.
///
/// The test that says there is one system: the two consumers differ in how they
/// *capture*, and in nothing else. Comparing two consumers over two
/// differently-loaded subjects would be asserting something else — the engine
/// loads the user's composite and the binary loads its own field.
#[test]
fn the_driver_is_sink_agnostic() {
    let mut off = Offscreen::new();
    let blocking = off.render("veils", "frozen");

    // The recording sink defers the readback the way the engine defers it to a
    // scheduler: finish and submit inside the capture, read back afterwards.
    let (device, queue) = (off.gpu.0.clone(), off.gpu.1.clone());
    let (w, h) = off.target.size();
    let mut outputs: Vec<wgpu::Texture> = Vec::new();
    {
        let Offscreen {
            target,
            veils,
            voids,
            filters,
            ..
        } = &mut off;
        let regs = PreviewRegistries {
            veils,
            voids,
            filters,
        };
        let (_, mech) = preview_mechanisms()
            .into_iter()
            .find(|(id, _)| *id == "veils")
            .unwrap();
        let mut seq = PreviewSequence::open(mech, regs, "frozen").unwrap();
        drive(
            &mut seq,
            &device,
            &queue,
            target,
            |encoder, output, _, _| {
                // Copy the frame aside inside the same submission that produced it,
                // which is the guarantee the engine's readback request relies on.
                let mut encoder = encoder;
                let (copy, _) = darkly::gpu::create_texture_with_view(
                    &device,
                    w,
                    h,
                    PREVIEW_FORMAT,
                    "sink-agnostic-copy",
                    wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
                );
                darkly::gpu::blit_region(&mut encoder, output, (0, 0), &copy, (0, 0), w, h);
                queue.submit([encoder.finish()]);
                outputs.push(copy);
            },
        );
    }
    let deferred: Vec<Vec<u8>> = outputs
        .iter()
        .map(|t| readback_texture(&device, &queue, t, PREVIEW_FORMAT, w, h))
        .collect();

    assert_eq!(
        blocking, deferred,
        "the same sequence produced different frames through two sinks"
    );
}

// ---------------------------------------------------------------------------
// The live consumer
// ---------------------------------------------------------------------------

/// `frozen` yields its full declared frame count with visible motion — the exact
/// defect of the shipped path, which showed one still at `strength = 0.04`.
#[test]
fn picker_preview_runs_the_animation_not_the_defaults() {
    let mut engine = headless_engine(256, 256);
    let anim = darkly::gpu::veil::VeilRegistry::new()
        .preview("frozen")
        .expect("frozen declares a preview");

    engine.start_preview("veils", "frozen");
    let (w, h, fps, frames) = drain_preview(&mut engine, "veils", "frozen");

    assert_eq!(frames.len(), anim.frames as usize);
    assert_eq!(fps, anim.fps, "the wire fps is the entry's own");
    assert!(w > 0 && h > 0);
    for (i, f) in frames.iter().enumerate() {
        assert_eq!(
            f.len(),
            (w * h * 4) as usize,
            "frame {i} is not packed RGBA8"
        );
    }
    assert!(
        frames.windows(2).any(|pair| pair[0] != pair[1]),
        "the frozen preview is still a single state repeated"
    );
}

/// A filter previews through the same offscreen path as a veil — a capability
/// the picker did not have at all before, and the second invocation contract
/// the mechanism trait has to satisfy.
#[test]
fn filter_picker_preview_renders_through_the_offscreen_path() {
    let mut engine = headless_engine(256, 256);
    engine.start_preview("filters", "hsv");
    let (w, h, _, frames) = drain_preview(&mut engine, "filters", "hsv");

    let anim = darkly::gpu::filter::FilterPipelineRegistry::new()
        .preview("hsv")
        .unwrap();
    assert_eq!(frames.len(), anim.frames as usize);
    assert_eq!(frames[0].len(), (w * h * 4) as usize);
    assert!(frames.windows(2).any(|pair| pair[0] != pair[1]));
}

/// `invert` takes no parameters, declares one frame, and yields exactly one.
#[test]
fn a_still_preview_is_one_frame() {
    let mut engine = headless_engine(256, 256);
    engine.start_preview("filters", "invert");
    let (w, h, _, frames) = drain_preview(&mut engine, "filters", "invert");
    assert_eq!(frames.len(), 1, "invert declares a still");
    assert_eq!(frames[0].len(), (w * h * 4) as usize);
}

/// A void generates its own content at the canvas's aspect-fit preview size.
#[test]
fn a_void_previews_from_scratch_at_the_canvas_aspect() {
    let mut engine = headless_engine(800, 400);
    engine.start_preview("voids", "noise");
    let (w, h, _, frames) = drain_preview(&mut engine, "voids", "noise");

    assert_eq!((w, h), fit_preview_dims(800, 400));
    assert!(w > h, "a wide canvas yields a wide preview");
    // Real content, not a blank buffer — and not a flat colour either.
    let first: &[u8] = &frames[0][..4];
    assert!(frames[0].chunks_exact(4).any(|px| px != first));
}

/// Generation is paced: `start_preview` produces nothing on its own, and no
/// single tick encodes more than the per-tick budget.
///
/// Frames in flight are unpooled `MAP_READ` staging buffers, so the budget is
/// what bounds the memory a picker open costs.
#[test]
fn a_preview_completes_across_ticks() {
    let mut engine = headless_engine(256, 256);
    engine.start_preview("veils", "frozen");
    assert!(
        engine.poll_preview("veils", "frozen").is_none(),
        "start_preview must enqueue, not generate"
    );

    let total = darkly::gpu::veil::VeilRegistry::new()
        .preview("frozen")
        .unwrap()
        .frames;
    // One tick cannot finish a sequence longer than the budget.
    assert!(total > PREVIEW_FRAMES_PER_TICK);
    engine.render(0.0);
    engine.test_flush_readbacks();
    assert!(
        engine.poll_preview("veils", "frozen").is_none(),
        "one tick encoded more than PREVIEW_FRAMES_PER_TICK frames"
    );

    let (_, _, _, frames) = drain_preview(&mut engine, "veils", "frozen");
    assert_eq!(frames.len(), total as usize);
}

/// Polling hands the job over rather than cloning it: a second poll answers
/// `None`, and the next open regenerates against the canvas as it then stands.
#[test]
fn polling_a_completed_preview_releases_it() {
    let mut engine = headless_engine(128, 128);
    engine.start_preview("filters", "invert");
    assert!(drain_preview(&mut engine, "filters", "invert").3.len() == 1);
    assert!(
        engine.poll_preview("filters", "invert").is_none(),
        "a completed preview must be handed over once"
    );

    engine.start_preview("filters", "invert");
    assert_eq!(drain_preview(&mut engine, "filters", "invert").3.len(), 1);
}

/// A request naming a catalog or a type the binary does not ship is a no-op —
/// the wire carries arbitrary strings, and there is nothing to render.
#[test]
fn an_unknown_catalog_or_type_is_a_no_op() {
    let mut engine = headless_engine(64, 64);
    for (catalog, type_id) in [
        ("nope", "frozen"),
        ("veils", "does_not_exist"),
        ("voids", "does_not_exist"),
        ("filters", "does_not_exist"),
        // A catalog with no offscreen mechanism: previewable as a documentation
        // asset, with nothing for a picker to open.
        ("blendModes", "multiply"),
    ] {
        engine.start_preview(catalog, type_id);
        engine.render(0.0);
        engine.test_flush_readbacks();
        assert!(
            engine.poll_preview(catalog, type_id).is_none(),
            "`{catalog}/{type_id}` produced a preview"
        );
    }
}

/// Generating a preview of any catalog leaves the document exactly as it was.
///
/// The isolation the whole offscreen path exists for: a preview builds its own
/// instance against its own textures, so the live veil chain, layer stack and
/// active layer are never touched.
#[test]
fn preview_generation_never_mutates_the_document() {
    let mut engine = headless_engine(128, 128);
    let before_layers = engine.layer_tree().len();
    let before_veils = engine.veil_list().len();

    for (catalog, type_id) in [
        ("veils", "black_and_white"),
        ("voids", "noise"),
        ("filters", "invert"),
    ] {
        engine.start_preview(catalog, type_id);
        drain_preview(&mut engine, catalog, type_id);
        assert_eq!(
            engine.layer_tree().len(),
            before_layers,
            "`{catalog}/{type_id}` changed the layer tree"
        );
        assert_eq!(
            engine.veil_list().len(),
            before_veils,
            "`{catalog}/{type_id}` changed the veil chain"
        );
    }
}
