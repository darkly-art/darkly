//! Matrix bench — replays a single recorded stroke through a headless
//! engine across a (dab radius × canvas resolution) grid and emits one
//! row of timings per cell. Brush, stabilization, and the axes are all
//! hardcoded near the top of this file so the matrix lives in one place;
//! `stroke_replay_bench` is the single-replay companion.
//!
//! Run with:
//!
//! ```bash
//! cargo run --release --bin stroke_replay_matrix -- \
//!     --input crates/darkly/tests/fixtures/recorded_curvy_stroke.json
//! ```
//!
//! Each cell:
//!   - Builds a fresh `DarklyEngine` at the cell's canvas size.
//!   - Loads `Ink Pen`, sets `pen_input.stabilize = 1.0` and the
//!     terminal's `size` port to the cell's dab radius.
//!   - Adds a raster layer.
//!   - Replays the recording at `ReplayPacing::Realtime`.
//!   - Records per-event CPU timings + total wall-clock.
//!
//! Note on `dab_radius_px`: this is the value of the brush graph's `size`
//! port at port-default-pressure (1.0). Ink Pen modulates `size_input`
//! through a pressure curve, so the *actual rendered* radius for the
//! recording's mouse-only pen (constant pressure = 0.5) will be ~71 % of
//! the column value. The matrix axis is the configured cap.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

use darkly::brush::builtin_brushes;
use darkly::brush::wire::BrushWireType;
use darkly::engine::DarklyEngine;
use darkly::format::stroke_recording::{replay, ReplayPacing, StrokeRecording};
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::bench_device;
use darkly::nodegraph::Graph;

// ── Matrix axes ─────────────────────────────────────────────────────────

/// Mirrors `crates/darkly/src/brush/dab_pool.rs::DAB_REFERENCE_SIZE`.
/// `radius_px = size_port * DAB_REFERENCE_SIZE_PX * 0.5`, so
/// `size_port = 2 * radius_px / DAB_REFERENCE_SIZE_PX`.
const DAB_REFERENCE_SIZE_PX: f32 = 512.0;

const DAB_RADII_PX: &[f32] = &[1.0, 10.0, 100.0, 1000.0, 2000.0];

const RESOLUTIONS: &[(u32, u32)] = &[(1280, 720), (1920, 1080), (2560, 1440), (3840, 2160)];

/// Built-in brush name — must match the string in `builtin_brushes::ink_pen()`.
const BRUSH_NAME: &str = "Ink Pen";

/// Stabilizer strength override. The recorded stroke is what stresses
/// the stabilizer; cranking this to 1.0 maximises the rewind workload.
const STABILIZE: f32 = 1.0;

// ── CLI ─────────────────────────────────────────────────────────────────

#[derive(Debug)]
struct Args {
    input: PathBuf,
    output: Option<PathBuf>,
}

fn parse_args() -> Args {
    let mut input: Option<PathBuf> = None;
    let mut output: Option<PathBuf> = None;
    let mut argv = std::env::args().skip(1);
    while let Some(a) = argv.next() {
        match a.as_str() {
            "--input" | "-i" => {
                input = Some(PathBuf::from(argv.next().expect("--input requires a path")));
            }
            "--output" | "-o" => {
                output = Some(PathBuf::from(
                    argv.next().expect("--output requires a path"),
                ));
            }
            "-h" | "--help" => {
                eprintln!(
                    "stroke_replay_matrix --input <path> [--output <tsv>]\n\n\
                     Replays a recording across the configured (dab_radius × resolution) matrix.\n\
                     Axes are constants at the top of stroke_replay_matrix.rs."
                );
                std::process::exit(0);
            }
            other => panic!("unknown arg: {other}"),
        }
    }
    Args {
        input: input.unwrap_or_else(|| {
            eprintln!("error: --input <path> is required");
            std::process::exit(2);
        }),
        output,
    }
}

// ── Brush graph customisation ───────────────────────────────────────────

fn customise_graph(graph: &mut Graph<BrushWireType>, dab_radius_px: f32, stabilize: f32) {
    let pen_id = graph
        .nodes
        .iter()
        .find(|(_, n)| n.type_id == "pen_input")
        .map(|(id, _)| *id)
        .expect("brush must have a pen_input node");
    let term_id = graph
        .nodes
        .iter()
        .find(|(_, n)| n.type_id == "paint_compute")
        .map(|(id, _)| *id)
        .expect("brush must have a paint_compute terminal");

    let size_port = (2.0 * dab_radius_px) / DAB_REFERENCE_SIZE_PX;
    graph
        .set_port_default(term_id, "size", size_port)
        .expect("set size port default");
    graph
        .set_port_default(pen_id, "stabilize", stabilize)
        .expect("set stabilize port default");
}

fn ink_pen_graph_json(dab_radius_px: f32) -> String {
    let mut brush = builtin_brushes::all()
        .into_iter()
        .find(|b| b.metadata.name == BRUSH_NAME)
        .unwrap_or_else(|| panic!("brush `{BRUSH_NAME}` not found in builtin_brushes::all()"));
    customise_graph(&mut brush.metadata.graph, dab_radius_px, STABILIZE);
    serde_json::to_string(&brush.metadata.graph).expect("serialize brush graph")
}

// ── Cell runner ─────────────────────────────────────────────────────────

#[derive(Debug)]
struct CellResult {
    canvas: (u32, u32),
    dab_radius_px: f32,
    event_count: u32,
    stroke_duration_ms: f64,
    wall_total_ms: f64,
    behind_by_ms: f64,
    cpu_median_us: f64,
    cpu_p95_us: f64,
    cpu_max_us: u64,
}

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

fn run_cell(recording: &StrokeRecording, canvas: (u32, u32), dab_radius_px: f32) -> CellResult {
    let graph_json = ink_pen_graph_json(dab_radius_px);
    let (device, queue) = bench_device();
    let gpu = GpuContext::new_headless(device, queue);
    let mut engine = DarklyEngine::new(gpu, canvas.0, canvas.1);
    engine
        .set_brush_graph(&graph_json)
        .expect("brush graph compiles");
    let layer_id = engine.add_raster_layer(None);

    let first_t = recording.events.first().expect("non-empty events").time_ms;
    let last_t = recording.events.last().unwrap().time_ms;
    let stroke_duration_ms = last_t - first_t;

    let wall_start = Instant::now();
    let timings = replay(
        &mut engine,
        recording,
        layer_id,
        canvas,
        ReplayPacing::Realtime,
    );
    let wall_total_ms = wall_start.elapsed().as_secs_f64() * 1000.0;

    let mut cpu_us: Vec<u64> = timings.iter().map(|t| t.cpu_us).collect();
    cpu_us.sort_unstable();

    CellResult {
        canvas,
        dab_radius_px,
        event_count: timings.len() as u32,
        stroke_duration_ms,
        wall_total_ms,
        behind_by_ms: wall_total_ms - stroke_duration_ms,
        cpu_median_us: percentile(&cpu_us, 0.5),
        cpu_p95_us: percentile(&cpu_us, 0.95),
        cpu_max_us: *cpu_us.last().unwrap_or(&0),
    }
}

// ── Output ──────────────────────────────────────────────────────────────

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
        .join(format!("stroke-replay-matrix-{stem}-{sha}.tsv"))
}

fn write_tsv(path: &Path, results: &[CellResult]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = fs::File::create(path)?;
    writeln!(
        file,
        "canvas_w\tcanvas_h\tdab_radius_px\tevent_count\tstroke_duration_ms\t\
         wall_total_ms\tbehind_by_ms\tcpu_median_us\tcpu_p95_us\tcpu_max_us"
    )?;
    for r in results {
        writeln!(
            file,
            "{}\t{}\t{}\t{}\t{:.3}\t{:.3}\t{:.3}\t{:.2}\t{:.2}\t{}",
            r.canvas.0,
            r.canvas.1,
            r.dab_radius_px,
            r.event_count,
            r.stroke_duration_ms,
            r.wall_total_ms,
            r.behind_by_ms,
            r.cpu_median_us,
            r.cpu_p95_us,
            r.cpu_max_us,
        )?;
    }
    Ok(())
}

fn write_markdown(
    path: &Path,
    recording: &StrokeRecording,
    results: &[CellResult],
) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = fs::File::create(path)?;
    writeln!(file, "# stroke_replay_matrix")?;
    writeln!(file)?;
    writeln!(
        file,
        "Brush: `{BRUSH_NAME}` (stabilize=`{STABILIZE}`). Recording: \
         {} events spanning {:.0} ms recorded at {}×{}. Replay pacing: \
         real-time. `behind_by_ms = wall_total - stroke_duration` — positive \
         means the engine fell behind the recorded cadence.",
        recording.events.len(),
        recording.events.last().unwrap().time_ms - recording.events[0].time_ms,
        recording.canvas_width,
        recording.canvas_height,
    )?;
    writeln!(file)?;
    writeln!(
        file,
        "| canvas | radius_px | events | duration (ms) | wall (ms) | behind (ms) | \
         cpu p50 (µs) | cpu p95 (µs) | cpu max (µs) |"
    )?;
    writeln!(file, "|---|---:|---:|---:|---:|---:|---:|---:|---:|")?;
    for r in results {
        writeln!(
            file,
            "| {}×{} | {} | {} | {:.0} | {:.0} | {:+.0} | {:.0} | {:.0} | {} |",
            r.canvas.0,
            r.canvas.1,
            r.dab_radius_px,
            r.event_count,
            r.stroke_duration_ms,
            r.wall_total_ms,
            r.behind_by_ms,
            r.cpu_median_us,
            r.cpu_p95_us,
            r.cpu_max_us,
        )?;
    }
    Ok(())
}

// ── Main ────────────────────────────────────────────────────────────────

fn main() {
    let args = parse_args();
    let recording = StrokeRecording::load(&args.input).unwrap_or_else(|e| {
        eprintln!("failed to load recording {}: {e}", args.input.display());
        std::process::exit(1);
    });
    eprintln!(
        "matrix: {} cells ({}×{}) on `{BRUSH_NAME}` stabilize={STABILIZE} \
         vs {} events spanning {:.0} ms",
        DAB_RADII_PX.len() * RESOLUTIONS.len(),
        DAB_RADII_PX.len(),
        RESOLUTIONS.len(),
        recording.events.len(),
        recording.events.last().unwrap().time_ms - recording.events[0].time_ms,
    );

    let mut results = Vec::new();
    for &canvas in RESOLUTIONS {
        for &dab_radius_px in DAB_RADII_PX {
            eprint!(
                "  canvas={}x{} radius={:>5}px ... ",
                canvas.0, canvas.1, dab_radius_px
            );
            let r = run_cell(&recording, canvas, dab_radius_px);
            eprintln!(
                "wall={:>5.0}ms ({:+5.0}ms vs stroke), cpu p50/p95/max = {:>5.0}/{:>5.0}/{:>6} µs",
                r.wall_total_ms, r.behind_by_ms, r.cpu_median_us, r.cpu_p95_us, r.cpu_max_us,
            );
            results.push(r);
        }
    }

    let tsv_path = args
        .output
        .unwrap_or_else(|| default_output_path(&args.input));
    let md_path = tsv_path.with_extension("md");
    match write_tsv(&tsv_path, &results) {
        Ok(_) => eprintln!("wrote {}", tsv_path.display()),
        Err(e) => eprintln!("failed to write {}: {e}", tsv_path.display()),
    }
    match write_markdown(&md_path, &recording, &results) {
        Ok(_) => eprintln!("wrote {}", md_path.display()),
        Err(e) => eprintln!("failed to write {}: {e}", md_path.display()),
    }
}
