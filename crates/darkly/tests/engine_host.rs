//! Tests for the engine host: the frame-driven executor, awaitable readbacks,
//! and the copy/cut tasks that run on them.
//!
//! These exercise the orchestration that drives a deferred op to completion
//! without a `pending_*` field or a central resume `match`: a copy/cut spawns a
//! task that awaits the selection cache (if cold) and the masked GPU readback,
//! re-acquiring the engine between awaits through the scoped `EngineCell::with`.
//!
//! Run with: `cargo test -p darkly --test engine_host --features testing -- --test-threads=1`

use std::rc::Rc;

use darkly::document::SelectionMode;
use darkly::engine::host::EngineHost;
use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::test_device;

fn test_engine(width: u32, height: u32) -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, width, height)
}

/// A solid red, fully-opaque RGBA buffer of `w × h`.
fn solid_red(w: u32, h: u32) -> Vec<u8> {
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for px in rgba.chunks_exact_mut(4) {
        px[0] = 255;
        px[3] = 255;
    }
    rgba
}

/// Copy with a **cold** selection CPU cache completes through the host: the task
/// warms the cache (cold after a boolean combine) before kicking the copy
/// readback, and the originating request resolves with the masked region.
#[test]
fn copy_with_cold_selection_cache_completes() {
    let (w, h) = (64u32, 64u32);
    let mut engine = test_engine(w, h);
    let layer = engine.paste_image(w, h, &solid_red(w, h), 0, 0, None);

    // A `Replace` selection warms the CPU cache directly, but a boolean combine
    // (`Add`) invalidates it and kicks an async readback — leaving the cache
    // cold until a frame poll lands it. The second rect sits inside the first,
    // so the union region is unchanged at [16,16,16,16].
    engine.select_rect(16.0, 16.0, 16.0, 16.0, SelectionMode::Replace, false, 0.0);
    engine.select_rect(20.0, 20.0, 8.0, 8.0, SelectionMode::Add, false, 0.0);
    assert!(
        engine.test_selection_cpu_cache().is_none(),
        "precondition: a combine leaves the selection CPU cache cold"
    );

    let host = EngineHost::adopt(engine);
    host.with(|e| e.copy(layer));
    host.pump_until_idle();

    let resp = host
        .with(|e| e.test_take_completed(0))
        .expect("cold-cache copy must resolve once the host drives its task");
    let export: darkly::engine::ClipboardExport =
        serde_json::from_value(resp.value).expect("copy resolves with a ClipboardExport");

    assert_eq!((export.width, export.height), (16, 16));
    assert_eq!((export.offset_x, export.offset_y), (16, 16));
    assert!(
        export.rgba.chunks_exact(4).all(|p| p[3] == 255),
        "every copied pixel inside the selection is opaque red"
    );
}

/// The cut GPU-math invariant: extracted (clipboard) + remaining (layer) ==
/// original. With a hard-edged selection, every selected pixel moves to the
/// clipboard (alpha 255) and leaves the layer transparent (alpha 0); unselected
/// pixels stay on the layer. Exercises the chained `.await` (cache → readback)
/// that replaced `pending_copy`.
#[test]
fn cut_extracted_plus_remaining_equals_original() {
    let (w, h) = (64u32, 64u32);
    let mut engine = test_engine(w, h);
    let layer = engine.paste_image(w, h, &solid_red(w, h), 0, 0, None);

    // Hard-edged (antialias=false) selection of the left-centre square.
    engine.select_rect(16.0, 16.0, 16.0, 16.0, SelectionMode::Replace, false, 0.0);

    let host = EngineHost::adopt(engine);
    host.with(|e| e.cut(layer));
    host.pump_until_idle();

    let resp = host
        .with(|e| e.test_take_completed(0))
        .expect("cut must resolve once the host drives its task");
    let export: darkly::engine::ClipboardExport =
        serde_json::from_value(resp.value).expect("cut resolves with a ClipboardExport");

    // Extracted: the 16×16 selected square, fully opaque.
    assert_eq!((export.width, export.height), (16, 16));
    assert!(
        export.rgba.chunks_exact(4).all(|p| p[3] == 255),
        "extracted square is fully opaque"
    );

    // Remaining: the layer is cleared inside the selection, intact outside.
    let engine = host.into_engine();
    let remaining = engine.test_readback_layer(layer);
    let alpha = |x: u32, y: u32| remaining[((y * w + x) * 4 + 3) as usize];
    assert_eq!(
        alpha(24, 24),
        0,
        "selected pixel must be cut from the layer"
    );
    assert_eq!(
        alpha(4, 4),
        255,
        "unselected pixel must remain on the layer"
    );
}

/// A task awaiting a readback must not block the frame loop. A single `tick`
/// with a copy in flight returns (the borrow-across-await deadlock would hang
/// here), and the copy still resolves once driven to completion.
#[test]
fn awaiting_task_does_not_block_the_frame_loop() {
    let (w, h) = (64u32, 64u32);
    let mut engine = test_engine(w, h);
    let layer = engine.paste_image(w, h, &solid_red(w, h), 0, 0, None);

    let host = EngineHost::adopt(engine);
    host.with(|e| e.copy(layer));

    // One frame with the task in flight: must return, not deadlock.
    let outcome = host.tick(0.0);
    assert!(!outcome.busy, "a plain frame should not report busy");

    host.pump_until_idle();
    assert!(
        host.with(|e| e.test_take_completed(0)).is_some(),
        "copy resolves after the frame loop drives it"
    );
}

/// Keepalive: a copy kicked with nothing else animating keeps `needsMore` true
/// until its task completes — otherwise the rAF loop would sleep and the promise
/// would hang (the readback is only driven by a frame).
#[test]
fn keepalive_holds_frames_until_copy_completes() {
    let (w, h) = (64u32, 64u32);
    let mut engine = test_engine(w, h);
    let layer = engine.paste_image(w, h, &solid_red(w, h), 0, 0, None);

    let host = EngineHost::adopt(engine);
    host.with(|e| e.copy(layer));

    // With the task in flight and no animation, the frame must report needsMore.
    let outcome = host.tick(0.0);
    assert!(
        outcome.needs_more,
        "a copy in flight must keep frames coming"
    );
    assert!(host.has_pending_tasks(), "the copy task is still in flight");

    host.pump_until_idle();
    assert!(
        host.with(|e| e.test_take_completed(0)).is_some(),
        "the copy eventually resolves"
    );
}

/// A destructive adjustment spawned with a **cold** selection cache resolves
/// through the host: the `run_adjustment` task warms the cache before applying
/// the filter, and only the selected region is inverted. Exercises the
/// `ensure_selection_cache_warm` combinator shared with copy/cut.
#[test]
fn adjustment_with_cold_selection_cache_resolves() {
    let (w, h) = (12u32, 12u32);
    let mut engine = test_engine(w, h);
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for (i, px) in rgba.chunks_exact_mut(4).enumerate() {
        px[0] = (i % 200) as u8;
        px[1] = 64;
        px[2] = 128;
        px[3] = 255;
    }
    let layer = engine.paste_image(w, h, &rgba, 0, 0, None);
    let before = engine.test_readback_layer(layer);

    // A boolean combine leaves the selection CPU cache cold (see the copy test).
    engine.select_rect(3.0, 3.0, 5.0, 5.0, SelectionMode::Replace, false, 0.0);
    engine.select_rect(4.0, 4.0, 3.0, 3.0, SelectionMode::Add, false, 0.0);
    assert!(
        engine.test_selection_cpu_cache().is_none(),
        "precondition: the combine leaves the cache cold"
    );

    let host = EngineHost::adopt(engine);
    host.with(|e| {
        e.test_set_request_id(0);
        e.spawn_adjustment(layer, "invert");
    });
    host.pump_until_idle();

    let resp = host
        .with(|e| e.test_take_completed(0))
        .expect("adjustment must resolve once the host drives its task");
    assert_eq!(resp.value["ok"], true, "adjustment reports ok");

    let after = host.with(|e| e.test_readback_layer(layer));
    let px = |buf: &[u8], x: u32, y: u32| {
        let i = ((y * w + x) * 4) as usize;
        [buf[i], buf[i + 1], buf[i + 2]]
    };
    // Inside the selection: inverted. Outside: untouched.
    let inv = |c: [u8; 3]| [255 - c[0], 255 - c[1], 255 - c[2]];
    assert_eq!(px(&after, 4, 4), inv(px(&before, 4, 4)));
    assert_eq!(px(&after, 0, 0), px(&before, 0, 0));
}

/// Flip and transform spawned through the host resolve their requests with
/// `{ ok: true }` — the deferred task path replacing the old `pending_flip` /
/// `pending_transform` + resume `match`.
#[test]
fn flip_and_transform_resolve_through_host() {
    let (w, h) = (32u32, 32u32);
    let mut engine = test_engine(w, h);
    let layer = engine.paste_image(w, h, &solid_red(w, h), 0, 0, None);

    let host = EngineHost::adopt(engine);

    host.with(|e| {
        e.test_set_request_id(1);
        e.spawn_flip(layer, darkly::gpu::ortho_transform::OrthoXform::FlipH);
    });
    host.pump_until_idle();
    assert_eq!(
        host.with(|e| e.test_take_completed(1))
            .expect("flip resolves")
            .value["ok"],
        true,
        "flip reports ok"
    );

    // No-selection transform drives the content-bounds compute inside the task.
    host.with(|e| {
        e.test_set_request_id(2);
        e.spawn_begin_transform(layer);
    });
    host.pump_until_idle();
    assert_eq!(
        host.with(|e| e.test_take_completed(2))
            .expect("transform resolves")
            .value["ok"],
        true,
        "transform sets up floating"
    );
    assert!(
        host.with(|e| e.has_floating()),
        "floating session is live after the transform task resolves"
    );
}

/// A re-entrant `EngineCell::with` while another burst holds the cell yields
/// `None` (the re-entrancy yield) instead of panicking and poisoning the cell.
/// Models the competing-borrow class headless; the real browser event-pump
/// re-entry is the manual repro.
#[test]
fn reentrant_with_yields_without_panic() {
    let host = EngineHost::adopt(test_engine(16, 16));
    let cell = host.cell().clone();

    let outer = cell.with(|_outer| {
        // Re-entrant acquire while the outer burst holds the borrow.
        cell.with(|_inner| 1)
    });

    assert_eq!(
        outer,
        Some(None),
        "outer burst runs (Some); the re-entrant burst yields (None)"
    );
}

/// Handle teardown with an in-flight task rejects its request (no dangling JS
/// promise) and drops the task, freeing the engine — the captured
/// `Rc<EngineCell>` is released, so the `Weak` no longer upgrades.
#[test]
fn dispose_rejects_inflight_task_and_frees_engine() {
    let (w, h) = (64u32, 64u32);
    let mut engine = test_engine(w, h);
    let layer = engine.paste_image(w, h, &solid_red(w, h), 0, 0, None);
    engine.select_rect(16.0, 16.0, 16.0, 16.0, SelectionMode::Replace, false, 0.0);

    let host = EngineHost::adopt(engine);
    let weak = Rc::downgrade(host.cell());

    // Spawn the copy task (request id 0) but do not drive it to completion.
    host.with(|e| e.copy(layer));

    let outcomes = host.dispose();
    assert!(
        outcomes.iter().any(|o| o.id == 0 && o.result.is_err()),
        "dispose rejects the in-flight copy's request"
    );

    drop(host);
    assert!(
        weak.upgrade().is_none(),
        "no task holds the engine cell after dispose — the engine is freed"
    );
}
