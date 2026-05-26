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
//!   - Loads the topology's brush (`Ink Pen` for paint-family
//!     topologies, `Smooth Watercolor` for the watercolor topology),
//!     sets `pen_input.stabilize = 1.0` and the terminal's `size`
//!     port to the cell's dab radius.
//!   - Adds a raster layer.
//!   - Replays the recording at `ReplayPacing::Realtime`.
//!   - Records per-event CPU + per-flush workload counters.
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
use darkly::engine::DarklyEngine;
use darkly::format::stroke_recording::{replay, ReplayPacing, StrokeRecording};
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::bench_device;

// ── Matrix axes ─────────────────────────────────────────────────────────

/// Mirrors `crates/darkly/src/brush/DAB_REFERENCE_SIZE`.
/// `radius_px = size_port * DAB_REFERENCE_SIZE_PX * 0.5`, so
/// `size_port = 2 * radius_px / DAB_REFERENCE_SIZE_PX`.
const DAB_REFERENCE_SIZE_PX: f32 = 512.0;

const DAB_RADII_PX: &[f32] = &[1.0, 10.0, 100.0, 250.0, 500.0, 1000.0, 2000.0];

const RESOLUTIONS: &[(u32, u32)] = &[(1280, 720), (1920, 1080), (2560, 1440), (3840, 2160)];

/// Built-in brush names — must match the strings in
/// `builtin_brushes::ink_pen()` and `builtin_brushes::smooth_watercolor()`.
/// `Topology::brush_name` picks the right one for the cell.
const BRUSH_NAME_INK_PEN: &str = "Ink Pen";
const BRUSH_NAME_WATERCOLOR: &str = "Smooth Watercolor";
const BRUSH_NAME_PERLIN_INK: &str = "Perlin Ink";

/// Stabilizer strength override. The recorded stroke is what stresses
/// the stabilizer; cranking this to 1.0 maximises the rewind workload.
const STABILIZE: f32 = 1.0;

// ── CLI ─────────────────────────────────────────────────────────────────

/// Which terminal topology the bench's brush graph uses for each cell.
/// All topologies run through compiled-WGSL terminals after the great
/// compiled-port migration — the `StampColorOutput` cross-approach
/// comparison was retired with the `color_output` dispatch terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Topology {
    /// Ink Pen brush through the compiled `paint_compiled` terminal —
    /// `pen → paint_color → circle (disc) → stamp → paint_compiled`.
    /// Single instanced render pass per phase.
    Paint,
    /// Wet Media (`Smooth Watercolor`) — `pen → paint_color → circle
    /// (sine) → watercolor_compiled`. Two-pass per phase (pickup atlas
    /// + composite), composite shader is per-brush compiled.
    Watercolor,
    /// Perlin Ink — `pen + 3×random → circle(perlin) → stamp →
    /// paint_compiled`. The original demo brush for the compiled
    /// framework; same terminal as Paint but a more elaborate upstream
    /// graph (per-dab random nodes drive the perlin shape).
    PerlinInk,
}

impl Topology {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "paint" => Some(Topology::Paint),
            "watercolor" | "watercolor-compute" | "wet-media" => Some(Topology::Watercolor),
            "perlin-ink" | "perlin_ink" | "compiled" => Some(Topology::PerlinInk),
            _ => None,
        }
    }

    fn slug(self) -> &'static str {
        match self {
            Topology::Paint => "paint",
            Topology::Watercolor => "watercolor",
            Topology::PerlinInk => "perlin-ink",
        }
    }

    /// Terminal node `type_id` to look up in the brush graph when
    /// overriding the cell's `size` port.
    fn terminal_id(self) -> &'static str {
        match self {
            Topology::Paint => "paint_compiled",
            Topology::Watercolor => "watercolor_compiled",
            Topology::PerlinInk => "paint_compiled",
        }
    }

    fn brush_name(self) -> &'static str {
        match self {
            Topology::Paint => BRUSH_NAME_INK_PEN,
            Topology::Watercolor => BRUSH_NAME_WATERCOLOR,
            Topology::PerlinInk => BRUSH_NAME_PERLIN_INK,
        }
    }
}

#[derive(Debug)]
struct Args {
    input: PathBuf,
    output: Option<PathBuf>,
    topology: Topology,
}

fn parse_args() -> Args {
    let mut input: Option<PathBuf> = None;
    let mut output: Option<PathBuf> = None;
    let mut topology = Topology::Paint;
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
            "--topology" | "-t" => {
                let v = argv.next().expect("--topology requires a value");
                topology = Topology::parse(&v).unwrap_or_else(|| {
                    panic!(
                        "unknown topology `{v}` — expected `paint`, `watercolor`, or `perlin-ink`"
                    )
                });
            }
            "-h" | "--help" => {
                eprintln!(
                    "stroke_replay_matrix --input <path> [--output <tsv>] \
                     [--topology paint|watercolor|perlin-ink]\n\n\
                     Replays a recording across the configured (dab_radius × resolution) matrix.\n\
                     Axes are constants at the top of stroke_replay_matrix.rs.\n\
                     `paint` = Ink Pen (compiled). `watercolor` = Smooth Watercolor (compiled).\n\
                     `perlin-ink` = the demo brush with the upstream random graph."
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
        topology,
    }
}

// ── Brush graph customisation ───────────────────────────────────────────

/// Load the topology's built-in brush, override its terminal's `size`
/// port and the `pen_input` stabilizer.
fn brush_graph_json(topology: Topology, dab_radius_px: f32) -> String {
    let brush_name = topology.brush_name();
    let terminal_id = topology.terminal_id();
    let mut brush = builtin_brushes::all()
        .into_iter()
        .find(|b| b.metadata.name == brush_name)
        .unwrap_or_else(|| panic!("brush `{brush_name}` not found in builtin_brushes::all()"));
    let graph = &mut brush.metadata.graph;
    let pen_id = graph
        .nodes
        .iter()
        .find(|(_, n)| n.type_id == "pen_input")
        .map(|(id, _)| *id)
        .expect("brush must have a pen_input node");
    let term_id = graph
        .nodes
        .iter()
        .find(|(_, n)| n.type_id == terminal_id)
        .map(|(id, _)| *id)
        .unwrap_or_else(|| panic!("brush `{brush_name}` must have a `{terminal_id}` terminal"));
    let size_port = (2.0 * dab_radius_px) / DAB_REFERENCE_SIZE_PX;
    graph
        .set_port_default(term_id, "size", size_port)
        .expect("set size port default");
    graph
        .set_port_default(pen_id, "stabilize", STABILIZE)
        .expect("set stabilize port default");
    serde_json::to_string(graph).expect("serialize brush graph")
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
    /// Worst single-event lateness: the max over events of
    /// `max(0, cpu_us/1000 - event_step_ms)`, where `event_step_ms` is the
    /// inter-event gap from the recording. Captures the worst dropped
    /// frame the user would have felt, where `behind_by_ms` only captures
    /// the cumulative stroke-level lag.
    max_event_behind_ms: f64,
    cpu_median_us: f64,
    cpu_p95_us: f64,
    cpu_max_us: u64,
    /// `queue.submit()` host time per event (back-pressure indicator).
    submit_median_us: f64,
    submit_p95_us: f64,
    submit_max_us: u64,
    /// Per-event averages of the workload the engine fed the GPU. `dabs/ev`
    /// discriminates spacing regimes; `bbox_area/ev` carries the union-
    /// bbox shape that mattered for the compute round-trip (and stays
    /// interesting for the fragment path's overdraw cost).
    dispatches_per_event_avg: f64,
    dabs_per_event_avg: f64,
    union_bbox_area_per_event_avg: f64,
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

fn run_cell(
    topology: Topology,
    recording: &StrokeRecording,
    canvas: (u32, u32),
    dab_radius_px: f32,
) -> CellResult {
    let graph_json = brush_graph_json(topology, dab_radius_px);
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

    let max_event_behind_ms = timings
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let step_ms = if i == 0 {
                0.0
            } else {
                timings[i].t_offset_ms - timings[i - 1].t_offset_ms
            };
            ((t.cpu_us as f64 / 1000.0) - step_ms).max(0.0)
        })
        .fold(0.0_f64, f64::max);

    let mut cpu_us: Vec<u64> = timings.iter().map(|t| t.cpu_us).collect();
    cpu_us.sort_unstable();

    let mut submit_us: Vec<u64> = timings.iter().map(|t| t.submit_us).collect();
    submit_us.sort_unstable();

    let total_events = timings.len().max(1) as f64;
    let dispatches_per_event_avg =
        timings.iter().map(|t| t.dab_flushes as f64).sum::<f64>() / total_events;
    let dabs_per_event_avg =
        timings.iter().map(|t| t.dabs_total as f64).sum::<f64>() / total_events;
    let union_bbox_area_per_event_avg = timings
        .iter()
        .map(|t| t.union_bbox_area_total as f64)
        .sum::<f64>()
        / total_events;

    CellResult {
        canvas,
        dab_radius_px,
        event_count: timings.len() as u32,
        stroke_duration_ms,
        wall_total_ms,
        behind_by_ms: wall_total_ms - stroke_duration_ms,
        max_event_behind_ms,
        cpu_median_us: percentile(&cpu_us, 0.5),
        cpu_p95_us: percentile(&cpu_us, 0.95),
        cpu_max_us: *cpu_us.last().unwrap_or(&0),
        submit_median_us: percentile(&submit_us, 0.5),
        submit_p95_us: percentile(&submit_us, 0.95),
        submit_max_us: *submit_us.last().unwrap_or(&0),
        dispatches_per_event_avg,
        dabs_per_event_avg,
        union_bbox_area_per_event_avg,
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

fn default_output_path(topology: Topology, input: &Path) -> PathBuf {
    let stem = input
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "recording".to_string());
    let sha = git_sha();
    let slug = topology.slug();
    workspace_root()
        .join("crates/darkly/bench-results")
        .join(format!("stroke-replay-matrix-{slug}-{stem}-{sha}.tsv"))
}

fn write_tsv(path: &Path, results: &[CellResult]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = fs::File::create(path)?;
    writeln!(
        file,
        "canvas_w\tcanvas_h\tdab_radius_px\tevent_count\tstroke_duration_ms\t\
         wall_total_ms\tbehind_by_ms\tmax_event_behind_ms\t\
         cpu_median_us\tcpu_p95_us\tcpu_max_us\t\
         submit_median_us\tsubmit_p95_us\tsubmit_max_us\t\
         dispatches_per_event_avg\tdabs_per_event_avg\tunion_bbox_area_per_event_avg"
    )?;
    for r in results {
        writeln!(
            file,
            "{}\t{}\t{}\t{}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\t\
             {:.2}\t{:.2}\t{}\t\
             {:.2}\t{:.2}\t{}\t\
             {:.3}\t{:.3}\t{:.0}",
            r.canvas.0,
            r.canvas.1,
            r.dab_radius_px,
            r.event_count,
            r.stroke_duration_ms,
            r.wall_total_ms,
            r.behind_by_ms,
            r.max_event_behind_ms,
            r.cpu_median_us,
            r.cpu_p95_us,
            r.cpu_max_us,
            r.submit_median_us,
            r.submit_p95_us,
            r.submit_max_us,
            r.dispatches_per_event_avg,
            r.dabs_per_event_avg,
            r.union_bbox_area_per_event_avg,
        )?;
    }
    Ok(())
}

fn write_markdown(
    topology: Topology,
    path: &Path,
    recording: &StrokeRecording,
    results: &[CellResult],
) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = fs::File::create(path)?;
    writeln!(file, "# stroke_replay_matrix — `{}`", topology.slug())?;
    writeln!(file)?;
    writeln!(
        file,
        "Brush: `{}` topology `{}` (terminal: `{}`, stabilize=`{STABILIZE}`). \
         Recording: {} events spanning {:.0} ms recorded at {}×{}. Replay pacing: \
         real-time. `behind_by_ms = wall_total - stroke_duration` — positive \
         means the engine fell behind the recorded cadence. \
         `max_event_behind_ms` is the worst single-event lateness \
         (`cpu_ms - inter_event_gap_ms`, clamped at zero, max across events).",
        topology.brush_name(),
        topology.slug(),
        topology.terminal_id(),
        recording.events.len(),
        recording.events.last().unwrap().time_ms - recording.events[0].time_ms,
        recording.canvas_width,
        recording.canvas_height,
    )?;
    writeln!(file)?;
    writeln!(
        file,
        "Markdown carries the slim view; the sibling TSV has p95/max for every column. \
         `submit` is host wall-clock around `queue.submit()` — high values indicate \
         back-pressure. `dispatches/ev`, `dabs/ev`, `bbox/ev` are per-event averages \
         of the workload the engine fed the GPU. The 6-slot GPU-timestamp columns \
         (`gpu_shader` / `gpu_sync_in` / `gpu_sync_out`) that the older matrices \
         carried are gone — they instrumented the compute-path buffer round-trip, \
         which the `paint` terminal no longer pays."
    )?;
    writeln!(file)?;
    writeln!(
        file,
        "| canvas | radius_px | events | wall (ms) | behind (ms) | worst-frame (ms) | \
         cpu p50 (µs) | submit p50 (µs) | dispatches/ev | dabs/ev | bbox px²/ev |"
    )?;
    writeln!(
        file,
        "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|"
    )?;
    for r in results {
        writeln!(
            file,
            "| {}×{} | {} | {} | {:.0} | {:+.0} | {:.1} | {:.0} | {:.0} | {:.2} | {:.1} | {:.0} |",
            r.canvas.0,
            r.canvas.1,
            r.dab_radius_px,
            r.event_count,
            r.wall_total_ms,
            r.behind_by_ms,
            r.max_event_behind_ms,
            r.cpu_median_us,
            r.submit_median_us,
            r.dispatches_per_event_avg,
            r.dabs_per_event_avg,
            r.union_bbox_area_per_event_avg,
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
        "matrix: {} cells ({}×{}) on `{}` topology=`{}` stabilize={STABILIZE} \
         vs {} events spanning {:.0} ms",
        DAB_RADII_PX.len() * RESOLUTIONS.len(),
        DAB_RADII_PX.len(),
        RESOLUTIONS.len(),
        args.topology.brush_name(),
        args.topology.slug(),
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
            let r = run_cell(args.topology, &recording, canvas, dab_radius_px);
            eprintln!(
                "wall={:>5.0}ms ({:+5.0}ms, worst-frame +{:>5.1}ms), \
                 cpu p50 = {:>5.0} µs, submit p50 = {:>5.0} µs, \
                 dabs/ev={:>4.1} bbox/ev={:>8.0}",
                r.wall_total_ms,
                r.behind_by_ms,
                r.max_event_behind_ms,
                r.cpu_median_us,
                r.submit_median_us,
                r.dabs_per_event_avg,
                r.union_bbox_area_per_event_avg,
            );
            results.push(r);
        }
    }

    let tsv_path = args
        .output
        .unwrap_or_else(|| default_output_path(args.topology, &args.input));
    let md_path = tsv_path.with_extension("md");
    match write_tsv(&tsv_path, &results) {
        Ok(_) => eprintln!("wrote {}", tsv_path.display()),
        Err(e) => eprintln!("failed to write {}: {e}", tsv_path.display()),
    }
    match write_markdown(args.topology, &md_path, &recording, &results) {
        Ok(_) => eprintln!("wrote {}", md_path.display()),
        Err(e) => eprintln!("failed to write {}: {e}", md_path.display()),
    }
}
