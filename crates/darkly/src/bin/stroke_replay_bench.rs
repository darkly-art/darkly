//! Stroke-replay bench harness — replays a recorded brush stroke through
//! a headless engine at the original real-time cadence and emits per-event
//! CPU timings.
//!
//! Companion to `frontend/src/lib/strokeRecorder.ts`. Where the recorder
//! captures raw tablet input (every pressure / tilt / timestamp sample),
//! this binary feeds the recording back into a fresh `DarklyEngine` so
//! the stabilizer and brush-engine pipeline run against real workloads
//! that exercise the stabilizer's direction-reversal rewind path.
//!
//! Run with:
//!
//! ```bash
//! cargo run --release --bin stroke_replay_bench -- \
//!     --input crates/darkly/tests/fixtures/stroke_recording_sample.json
//! ```
//!
//! CLI flags:
//!
//! - `--input <path>` (required) — recording file produced by the frontend recorder.
//! - `--brush <name>` (default `round`) — name from `builtin_brushes::all()`.
//! - `--dab-size <px>` — override the terminal's `size` port for the replay.
//! - `--canvas <WxH>` — override the engine canvas dims; recorded `(x, y)`
//!   are scaled by `target / recording.canvas_*` so the stroke fills the
//!   same fraction of the canvas.
//! - `--output <path>` — TSV output path. Defaults to
//!   `crates/darkly/bench-results/stroke-replay-<stem>-<sha>.tsv`.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

use darkly::brush::builtin_brushes;
use darkly::engine::DarklyEngine;
use darkly::format::stroke_recording::{replay, EventTiming, ReplayPacing, StrokeRecording};
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::test_device;

/// Kept in lockstep with `crates/darkly/src/brush/dab_pool.rs::DAB_REFERENCE_SIZE`.
/// The terminal node's `size` port is the dab radius expressed as a fraction
/// of this reference: `radius_px = size_port * DAB_REFERENCE_SIZE_PX * 0.5`,
/// so `--dab-size <px>` inverts to `size_port = 2 * px / DAB_REFERENCE_SIZE_PX`.
const DAB_REFERENCE_SIZE_PX: f32 = 512.0;

#[derive(Debug)]
struct Args {
    input: PathBuf,
    brush: String,
    dab_size_px: Option<f32>,
    canvas: Option<(u32, u32)>,
    output: Option<PathBuf>,
}

fn parse_args() -> Args {
    let mut args = Args {
        input: PathBuf::new(),
        brush: "round".to_string(),
        dab_size_px: None,
        canvas: None,
        output: None,
    };
    let mut input_set = false;
    let mut argv = std::env::args().skip(1);
    while let Some(a) = argv.next() {
        match a.as_str() {
            "--input" | "-i" => {
                args.input = PathBuf::from(argv.next().expect("--input requires a path"));
                input_set = true;
            }
            "--brush" | "-b" => {
                args.brush = argv.next().expect("--brush requires a name");
            }
            "--dab-size" | "-d" => {
                let v: f32 = argv
                    .next()
                    .expect("--dab-size requires a value")
                    .parse()
                    .expect("--dab-size must be a number");
                args.dab_size_px = Some(v);
            }
            "--canvas" | "-c" => {
                let v = argv.next().expect("--canvas requires WxH");
                args.canvas = Some(parse_canvas(&v));
            }
            "--output" | "-o" => {
                args.output = Some(PathBuf::from(
                    argv.next().expect("--output requires a path"),
                ));
            }
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            other => panic!("unknown arg: {other}"),
        }
    }
    if !input_set {
        eprintln!("error: --input <path> is required");
        print_help();
        std::process::exit(2);
    }
    args
}

fn parse_canvas(s: &str) -> (u32, u32) {
    let (w, h) = s
        .split_once(['x', 'X'])
        .unwrap_or_else(|| panic!("--canvas expects WxH (got `{s}`)"));
    (
        w.parse().expect("canvas width must be a u32"),
        h.parse().expect("canvas height must be a u32"),
    )
}

fn print_help() {
    eprintln!(
        "stroke_replay_bench --input <path> [--brush <name>] [--dab-size <px>] \
         [--canvas <WxH>] [--output <tsv>]"
    );
}

// ── Brush graph lookup + size override ──────────────────────────────────

fn brush_graph_json(brush_name: &str, dab_size_px: Option<f32>) -> String {
    let mut brush = builtin_brushes::all()
        .into_iter()
        .find(|b| b.metadata.name.eq_ignore_ascii_case(brush_name))
        .unwrap_or_else(|| {
            eprintln!(
                "error: brush `{brush_name}` not found. Available: {}",
                builtin_brushes::all()
                    .iter()
                    .map(|b| b.metadata.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            std::process::exit(2);
        });

    if let Some(radius_px) = dab_size_px {
        let size_port_value = (2.0 * radius_px) / DAB_REFERENCE_SIZE_PX;
        let terminal_id = brush
            .metadata
            .graph
            .nodes
            .iter()
            .find(|(_, n)| matches!(n.type_id.as_str(), "paint" | "watercolor"))
            .map(|(id, _)| *id)
            .unwrap_or_else(|| {
                panic!("brush `{brush_name}` has no compiled paint/watercolor terminal")
            });
        brush
            .metadata
            .graph
            .set_port_default(terminal_id, "size", size_port_value)
            .expect("set size port default");
    }

    serde_json::to_string(&brush.metadata.graph).expect("serialize brush graph")
}

// ── Engine setup ────────────────────────────────────────────────────────

fn build_engine(canvas: (u32, u32)) -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, canvas.0, canvas.1)
}

// ── Output ──────────────────────────────────────────────────────────────

fn percentile(sorted: &[u64], pct: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((sorted.len() as f64) * pct).clamp(0.0, sorted.len() as f64 - 1.0);
    let lo = idx.floor() as usize;
    let hi = idx.ceil() as usize;
    if lo == hi {
        return sorted[lo] as f64;
    }
    let frac = idx - lo as f64;
    sorted[lo] as f64 * (1.0 - frac) + sorted[hi] as f64 * frac
}

fn git_sha() -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "--short=10", "HEAD"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

fn workspace_root() -> PathBuf {
    let manifest = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest)
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from(manifest))
}

fn default_output_path(input: &Path) -> PathBuf {
    let stem = input
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "recording".to_string());
    let sha = git_sha();
    workspace_root()
        .join("crates/darkly/bench-results")
        .join(format!("stroke-replay-{stem}-{sha}.tsv"))
}

fn write_tsv(path: &Path, timings: &[EventTiming]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = fs::File::create(path)?;
    writeln!(
        file,
        "ev_index\tt_offset_ms\tcpu_us\t\
         submit_us\tsubmits\tdab_flushes\tdabs_total\tunion_bbox_area_total"
    )?;
    for t in timings {
        writeln!(
            file,
            "{}\t{:.3}\t{}\t{}\t{}\t{}\t{}\t{}",
            t.index,
            t.t_offset_ms,
            t.cpu_us,
            t.submit_us,
            t.submits,
            t.dab_flushes,
            t.dabs_total,
            t.union_bbox_area_total,
        )?;
    }
    Ok(())
}

// ── main ────────────────────────────────────────────────────────────────

fn main() {
    let args = parse_args();
    let recording = StrokeRecording::load(&args.input).unwrap_or_else(|e| {
        eprintln!("failed to load recording {}: {e}", args.input.display());
        std::process::exit(1);
    });

    let target_canvas = args
        .canvas
        .unwrap_or((recording.canvas_width, recording.canvas_height));

    let graph_json = brush_graph_json(&args.brush, args.dab_size_px);

    let mut engine = build_engine(target_canvas);
    engine
        .set_brush_graph(&graph_json)
        .expect("brush graph compiles");
    let layer_id = engine.add_raster_layer(None);

    eprintln!(
        "replaying {} ({} events) -> brush={} canvas={}x{} dab_size={:?}",
        args.input.display(),
        recording.events.len(),
        args.brush,
        target_canvas.0,
        target_canvas.1,
        args.dab_size_px,
    );

    let wall_start = Instant::now();
    let timings = replay(
        &mut engine,
        &recording,
        layer_id,
        target_canvas,
        ReplayPacing::Realtime,
    );
    let wall_elapsed_ms = wall_start.elapsed().as_secs_f64() * 1000.0;

    let mut cpu_us: Vec<u64> = timings.iter().map(|t| t.cpu_us).collect();
    cpu_us.sort_unstable();
    let total_cpu_us: u64 = cpu_us.iter().sum();
    let median_us = percentile(&cpu_us, 0.5);
    let p95_us = percentile(&cpu_us, 0.95);

    let mut submit_us_sorted: Vec<u64> = timings.iter().map(|t| t.submit_us).collect();
    submit_us_sorted.sort_unstable();

    eprintln!(
        "replayed {} events in {:.1} ms wall (cpu_total={} µs, cpu_median={:.0} µs, \
         cpu_p95={:.0} µs, submit_p50 = {:.0} µs)",
        timings.len(),
        wall_elapsed_ms,
        total_cpu_us,
        median_us,
        p95_us,
        percentile(&submit_us_sorted, 0.5),
    );

    let output = args
        .output
        .unwrap_or_else(|| default_output_path(&args.input));
    match write_tsv(&output, &timings) {
        Ok(_) => eprintln!("wrote {}", output.display()),
        Err(e) => eprintln!("failed to write {}: {e}", output.display()),
    }
}
