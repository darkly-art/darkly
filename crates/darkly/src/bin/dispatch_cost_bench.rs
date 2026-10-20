//! Dispatch-cost harness: what does one dab cost as a compute dispatch?
//!
//! Tests the hypothesis in section F of
//! `docs/paint-compute-perf-tracking.md`: that a
//! paint terminal issuing **one compute dispatch per dab inside a single
//! compute pass**, against a stroke-resident read-write `r32uint` storage
//! texture, keeps up where a render pass per dab (attempt #1, and the
//! shipped read-mirror terminals) collapses.
//!
//! The one quantity nobody had measured is the all-in cost of such a
//! dispatch, barrier included, at a thousand per pass. This binary measures
//! it directly, with no engine and no stroke, in five shapes over the same
//! N dabs scattered on a 3840x2160 target:
//!
//! - **(a) dispatch, read-write**: one compute pass, N dispatches, each
//!   unpack / source-over / pack on the `r32uint` texture. The hypothesis.
//! - **(b) dispatch, read-only**: the same N dispatches with the texture
//!   bound read-only and the store dropped. Identical CPU encode, no
//!   inter-dispatch barrier; (a) minus (b) is the barrier's share.
//! - **(c) render pass per dab**: N single-instance render passes onto an
//!   `rgba8unorm` attachment with hardware source-over. The shape the
//!   smudge pays today, reproduced on this machine in this harness.
//!
//! Two variants of (a) size the large-dab mitigations the compute paint
//! terminal can choose between (the port plan's section on the large-dab
//! regime): **(a16)**, the same shader from a 16x16 workgroup, and
//! **(a-rows)**, an 8x2 workgroup whose threads each walk four rows. Both
//! run on every cell, so the small-dab cost of each is measured beside its
//! large-dab gain.
//!
//! Every shape hands its dispatch or draw its dab through the same
//! mechanism the terminal would use: a static index buffer (slot `i` holds
//! `i`, written once) bound with a dynamic offset, and a tight record array
//! uploaded once per iteration.
//!
//! Each cell warms up untimed, then times the shapes interleaved
//! within every iteration, so an integrated GPU's frequency scaling lands
//! on all three alike; p50 and min are both reported.
//!
//! Timing is wall clock from encode start until `device.poll(Wait)` returns,
//! which is legal here (a bench binary under the `testing` feature; see
//! `docs/lessons-learned/gpu-lessons-learned.md` section 5), plus pass
//! timestamps when the adapter offers `TIMESTAMP_QUERY`. After each cell,
//! (a) and (c) are run once more from a cleared target and compared, so the
//! timings are of equivalent work.
//!
//! Run with:
//!
//! ```bash
//! cargo run --release -p darkly --features testing --bin dispatch_cost_bench
//! ```
//!
//! No CLI: the axes are constants, like `stroke_replay_matrix`. Output goes
//! to stdout and to `crates/darkly/bench-results/dispatch-cost-bench-<sha>.md`.

use std::fs;
use std::io::Write as _;
use std::num::NonZeroU64;
use std::path::PathBuf;
use std::time::Instant;

use darkly::gpu::test_utils::bench_device;

const TARGET_W: u32 = 3840;
const TARGET_H: u32 = 2160;
/// The cells, as `(radius_px, dab_count)`. The small radii (a 3x3
/// footprint in one workgroup, and a 3x3 grid of workgroups) at the
/// matrix's many-dabs-per-event counts measure per-dispatch overhead; the
/// large radius at the matrix's few-dabs-per-event counts measures the
/// thread-per-pixel cost of a dab that covers most of a 4K canvas.
const CELLS: [(f32, u32); 8] = [
    (1.5, 300),
    (1.5, 900),
    (1.5, 2000),
    (10.0, 300),
    (10.0, 900),
    (10.0, 2000),
    (1000.0, 5),
    (1000.0, 10),
];
const ITERATIONS: usize = 20;
/// Untimed iterations of every shape before a cell is timed.
const WARMUP: usize = 5;
const MAX_DABS: u32 = 2000;
/// Stride of the static index buffer: WebGPU's default
/// `min_uniform_buffer_offset_alignment`, which is what the web pays.
const INDEX_STRIDE: u64 = 256;
const RECORD_SIZE: u64 = 32;
const WORKGROUP: u32 = 8;

/// One dab, 32 bytes, matching the WGSL `Dab` struct.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Dab {
    pos: [f32; 2],
    radius: f32,
    _pad: f32,
    color: [f32; 4],
}

const SHADER: &str = r#"
struct Dab {
    pos: vec2<f32>,
    radius: f32,
    pad: f32,
    color: vec4<f32>,
}

struct Slot {
    i: u32,
    pad: u32,
    target_size: vec2<f32>,
}

@group(0) @binding(0) var<uniform> slot: Slot;
@group(0) @binding(1) var<storage, read> dabs: array<Dab>;
@group(0) @binding(2) var ground_rw: texture_storage_2d<r32uint, read_write>;
@group(0) @binding(3) var ground_ro: texture_2d<u32>;

fn coverage(dab: Dab, center: vec2<f32>) -> f32 {
    let d = distance(center, dab.pos) / dab.radius;
    return smoothstep(1.0, 0.6, d);
}

fn pixel_of(dab: Dab, local: vec2<u32>) -> vec2<i32> {
    let origin = vec2<i32>(floor(dab.pos - vec2<f32>(dab.radius)));
    return origin + vec2<i32>(local);
}

fn inside(px: vec2<i32>, dims: vec2<i32>) -> bool {
    return px.x >= 0 && px.y >= 0 && px.x < dims.x && px.y < dims.y;
}

// (a): read, blend, write, all on the read-write storage texture.
@compute @workgroup_size(8, 8, 1)
fn cs_rw(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dab = dabs[slot.i];
    let px = pixel_of(dab, gid.xy);
    if (!inside(px, vec2<i32>(textureDimensions(ground_rw)))) { return; }
    let c = coverage(dab, vec2<f32>(px) + vec2<f32>(0.5));
    if (c <= 0.0) { return; }
    let src = dab.color * c;
    let dst = unpack4x8unorm(textureLoad(ground_rw, px).r);
    let out = src + dst * (1.0 - src.a);
    textureStore(ground_rw, px, vec4<u32>(pack4x8unorm(out), 0u, 0u, 0u));
}

// (a16): the same read-modify-write from a 16x16 workgroup, exactly
// `max_compute_invocations_per_workgroup`.
@compute @workgroup_size(16, 16, 1)
fn cs_rw_16(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dab = dabs[slot.i];
    let px = pixel_of(dab, gid.xy);
    if (!inside(px, vec2<i32>(textureDimensions(ground_rw)))) { return; }
    let c = coverage(dab, vec2<f32>(px) + vec2<f32>(0.5));
    if (c <= 0.0) { return; }
    let src = dab.color * c;
    let dst = unpack4x8unorm(textureLoad(ground_rw, px).r);
    let out = src + dst * (1.0 - src.a);
    textureStore(ground_rw, px, vec4<u32>(pack4x8unorm(out), 0u, 0u, 0u));
}

// (a-rows): the same read-modify-write with each thread walking four rows,
// so an 8x2 workgroup covers the 8x8 tile of (a) with a quarter of the
// threads and one record load per four pixels.
@compute @workgroup_size(8, 2, 1)
fn cs_rw_rows(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dab = dabs[slot.i];
    let dims = vec2<i32>(textureDimensions(ground_rw));
    for (var r = 0u; r < 4u; r++) {
        let px = pixel_of(dab, vec2<u32>(gid.x, gid.y * 4u + r));
        if (!inside(px, dims)) { continue; }
        let c = coverage(dab, vec2<f32>(px) + vec2<f32>(0.5));
        if (c <= 0.0) { continue; }
        let src = dab.color * c;
        let dst = unpack4x8unorm(textureLoad(ground_rw, px).r);
        let out = src + dst * (1.0 - src.a);
        textureStore(ground_rw, px, vec4<u32>(pack4x8unorm(out), 0u, 0u, 0u));
    }
}

// (b): the same read and blend from a read-only binding, no store. The
// comparison against an impossible value keeps the load alive.
@compute @workgroup_size(8, 8, 1)
fn cs_ro(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dab = dabs[slot.i];
    let px = pixel_of(dab, gid.xy);
    if (!inside(px, vec2<i32>(textureDimensions(ground_ro)))) { return; }
    let c = coverage(dab, vec2<f32>(px) + vec2<f32>(0.5));
    if (c <= 0.0) { return; }
    let src = dab.color * c;
    let dst = unpack4x8unorm(textureLoad(ground_ro, px, 0).r);
    let out = src + dst * (1.0 - src.a);
    if (out.a > dab.radius + 1.0e9) { textureStore(ground_rw, px, vec4<u32>(0u)); }
}

// (c): one instanced quad per pass, hardware source-over.
struct VsOut {
    @builtin(position) position: vec4<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    let dab = dabs[slot.i];
    let corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    );
    let corner = corners[vi];
    let half = ceil(dab.radius) + 1.0;
    let origin = floor(dab.pos - vec2<f32>(dab.radius));
    let px = origin + corner * (2.0 * half);
    let ndc = vec2<f32>(px.x / slot.target_size.x * 2.0 - 1.0, 1.0 - px.y / slot.target_size.y * 2.0);
    var out: VsOut;
    out.position = vec4<f32>(ndc, 0.0, 1.0);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let dab = dabs[slot.i];
    let c = coverage(dab, in.position.xy);
    if (c <= 0.0) { discard; }
    return dab.color * c;
}
"#;

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    compute_rw: wgpu::ComputePipeline,
    compute_rw_16: wgpu::ComputePipeline,
    compute_rw_rows: wgpu::ComputePipeline,
    compute_ro: wgpu::ComputePipeline,
    render: wgpu::RenderPipeline,
    /// Two bind groups over one layout, because wgpu tracks a bind group's
    /// usages whole: the ground cannot be bound read-write and sampled in
    /// the same group, so each group binds the ground one way and a dummy
    /// the other way.
    bind_group_rw: wgpu::BindGroup,
    bind_group_ro: wgpu::BindGroup,
    records: wgpu::Buffer,
    ground: wgpu::Texture,
    attachment: wgpu::Texture,
    attachment_view: wgpu::TextureView,
    timestamps: Option<Timestamps>,
}

struct Timestamps {
    query_set: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    map: wgpu::Buffer,
    period_ns: f32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    DispatchRw,
    /// (a) from a 16x16 workgroup.
    DispatchRw16,
    /// (a) with each thread walking four rows from an 8x2 workgroup.
    DispatchRwRows,
    DispatchRo,
    RenderPass,
}

impl Shape {
    fn label(self) -> &'static str {
        match self {
            Shape::DispatchRw => "(a) dispatch, read-write",
            Shape::DispatchRw16 => "(a16) dispatch, read-write, 16x16",
            Shape::DispatchRwRows => "(a-rows) dispatch, read-write, 4 rows per thread",
            Shape::DispatchRo => "(b) dispatch, read-only",
            Shape::RenderPass => "(c) render pass per dab",
        }
    }

    /// Pixels per workgroup along each axis of the dispatch grid: the
    /// side of the tile one workgroup covers, whatever its thread count.
    fn tile(self) -> u32 {
        match self {
            Shape::DispatchRw16 => 16,
            _ => WORKGROUP,
        }
    }
}

#[derive(Clone, Copy)]
struct Sample {
    wall_ms: f64,
    gpu_ms: Option<f64>,
}

fn main() {
    let gpu = Gpu::new();
    let info = gpu.device.adapter_info();
    let mut out = String::new();
    let mut line = |s: String| {
        println!("{s}");
        out.push_str(&s);
        out.push('\n');
    };
    line(format!("# dispatch_cost_bench at `{}`", git_sha()));
    line(String::new());
    line(format!(
        "Adapter: {} ({:?}, driver {} {}). Target {}x{}. {} iterations per cell, p50 reported. Pass timestamps: {}.",
        info.name,
        info.backend,
        info.driver,
        info.driver_info,
        TARGET_W,
        TARGET_H,
        ITERATIONS,
        if gpu.timestamps.is_some() { "yes" } else { "no" },
    ));
    line(String::new());
    line("| radius_px | N | shape | wall p50 (ms) | wall min (ms) | wall per dab, p50 (us) | gpu p50 (ms) | gpu per dab, p50 (us) | parity max diff (LSB) |".into());
    line("|---:|---:|---|---:|---:|---:|---:|---:|---:|".into());

    let shapes = [
        Shape::DispatchRw,
        Shape::DispatchRw16,
        Shape::DispatchRwRows,
        Shape::DispatchRo,
        Shape::RenderPass,
    ];
    for &(radius, n) in &CELLS {
        {
            let dabs = scatter(n, radius);
            let parity = gpu.parity(&dabs);
            // Untimed warm-up so the first timed iteration does not pay
            // pipeline warm-up or an idle GPU clock, then the shapes
            // interleaved within each iteration so all three see the same
            // clock state: an integrated GPU ramps its frequency with load,
            // and running one shape's twenty iterations back to back would
            // hand the later shapes a warmer clock.
            for _ in 0..WARMUP {
                for shape in shapes {
                    gpu.time_once(shape, &dabs);
                }
            }
            let mut samples: Vec<Vec<Sample>> = vec![Vec::new(); shapes.len()];
            for _ in 0..ITERATIONS {
                for (k, shape) in shapes.iter().enumerate() {
                    samples[k].push(gpu.time_once(*shape, &dabs));
                }
            }
            for (k, shape) in shapes.iter().enumerate() {
                let mut walls: Vec<f64> = samples[k].iter().map(|s| s.wall_ms).collect();
                walls.sort_by(|x, y| x.total_cmp(y));
                let wall = walls[walls.len() / 2];
                let wall_min = walls[0];
                let mut gpu_times: Vec<f64> = samples[k].iter().filter_map(|s| s.gpu_ms).collect();
                gpu_times.sort_by(|x, y| x.total_cmp(y));
                let gpu_p50 = gpu_times.get(gpu_times.len() / 2).copied();
                let per_dab = |ms: f64| ms * 1000.0 / n as f64;
                line(format!(
                    "| {radius} | {n} | {} | {wall:.2} | {wall_min:.2} | {:.1} | {} | {} | {} |",
                    shape.label(),
                    per_dab(wall),
                    gpu_p50
                        .map(|g| format!("{g:.2}"))
                        .unwrap_or_else(|| "-".into()),
                    gpu_p50
                        .map(|g| format!("{:.1}", per_dab(g)))
                        .unwrap_or_else(|| "-".into()),
                    if *shape == Shape::DispatchRw {
                        parity.to_string()
                    } else {
                        "-".into()
                    },
                ));
            }
        }
    }

    let path = output_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create bench-results dir");
    }
    let mut file = fs::File::create(&path).expect("create results file");
    file.write_all(out.as_bytes()).expect("write results");
    println!("\nwrote {}", path.display());
}

/// Deterministic scatter of `n` dabs over the target, away from the edges
/// so every footprint lies inside it. A fixed LCG so every run and every
/// shape sees the same positions.
fn scatter(n: u32, radius: f32) -> Vec<Dab> {
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = || {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((state >> 33) as f64) / ((1u64 << 31) as f64)
    };
    // Large dabs may hang off the target; the shader clips them.
    let margin = (radius.ceil() + 2.0).min(TARGET_H as f32 / 2.0 - 8.0);
    (0..n)
        .map(|i| {
            let x = margin + next() as f32 * (TARGET_W as f32 - 2.0 * margin);
            let y = margin + next() as f32 * (TARGET_H as f32 - 2.0 * margin);
            let t = i as f32 / n as f32;
            Dab {
                pos: [x, y],
                radius,
                _pad: 0.0,
                color: [0.2 + 0.6 * t, 0.5, 0.8 - 0.6 * t, 1.0],
            }
        })
        .collect()
}

impl Gpu {
    fn new() -> Self {
        let (device, queue) = bench_device();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("dispatch-cost-bench"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("dispatch-cost-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE | wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: NonZeroU64::new(16),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE | wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::ReadWrite,
                        format: wgpu::TextureFormat::R32Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("dispatch-cost-layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let compute = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let compute_rw = compute("cs_rw");
        let compute_rw_16 = compute("cs_rw_16");
        let compute_rw_rows = compute("cs_rw_rows");
        let compute_ro = compute("cs_ro");
        let render = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("render-pass-per-dab"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        // The static index buffer: slot i holds i, plus the target size,
        // written once here and never again.
        let mut index_bytes = vec![0u8; (INDEX_STRIDE * MAX_DABS as u64) as usize];
        for i in 0..MAX_DABS {
            let at = (i as u64 * INDEX_STRIDE) as usize;
            index_bytes[at..at + 4].copy_from_slice(&i.to_le_bytes());
            index_bytes[at + 8..at + 12].copy_from_slice(&(TARGET_W as f32).to_le_bytes());
            index_bytes[at + 12..at + 16].copy_from_slice(&(TARGET_H as f32).to_le_bytes());
        }
        let index = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("dab-index"),
            size: index_bytes.len() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&index, 0, &index_bytes);
        let records = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("dab-records"),
            size: RECORD_SIZE * MAX_DABS as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let texture = |label: &str, format: wgpu::TextureFormat, usage: wgpu::TextureUsages| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: TARGET_W,
                    height: TARGET_H,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let ground = texture(
            "ground-r32uint",
            wgpu::TextureFormat::R32Uint,
            wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC,
        );
        let attachment = texture(
            "attachment-rgba8",
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let dummy = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("dummy-r32uint"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Uint,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let ground_view = ground.create_view(&Default::default());
        let dummy_view = dummy.create_view(&Default::default());
        let attachment_view = attachment.create_view(&Default::default());
        let make_bind_group = |label: &str, rw: &wgpu::TextureView, ro: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &index,
                            offset: 0,
                            size: NonZeroU64::new(16),
                        }),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: records.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(rw),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(ro),
                    },
                ],
            })
        };
        let bind_group_rw = make_bind_group("bg-read-write", &ground_view, &dummy_view);
        let bind_group_ro = make_bind_group("bg-read-only", &dummy_view, &ground_view);

        let timestamps = device
            .features()
            .contains(wgpu::Features::TIMESTAMP_QUERY)
            .then(|| Timestamps {
                query_set: device.create_query_set(&wgpu::QuerySetDescriptor {
                    label: Some("pass-timestamps"),
                    ty: wgpu::QueryType::Timestamp,
                    count: 2,
                }),
                resolve: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("timestamp-resolve"),
                    size: 16,
                    usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                }),
                map: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("timestamp-map"),
                    size: 16,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                period_ns: queue.get_timestamp_period(),
            });

        Self {
            device,
            queue,
            compute_rw,
            compute_rw_16,
            compute_rw_rows,
            compute_ro,
            render,
            bind_group_rw,
            bind_group_ro,
            records,
            ground,
            attachment,
            attachment_view,
            timestamps,
        }
    }

    /// Clear both targets in their own submission, outside any timing.
    fn clear(&self) {
        let mut encoder = self.device.create_command_encoder(&Default::default());
        for view in [
            self.ground.create_view(&Default::default()),
            self.attachment_view.clone(),
        ] {
            encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
        }
        self.queue.submit([encoder.finish()]);
        self.wait();
    }

    fn wait(&self) {
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("device poll");
    }

    /// One timed run of `shape` over `dabs`: upload, encode, submit, wait.
    fn time_once(&self, shape: Shape, dabs: &[Dab]) -> Sample {
        self.clear();
        let start = Instant::now();
        self.queue
            .write_buffer(&self.records, 0, bytemuck::cast_slice(dabs));
        let mut encoder = self.device.create_command_encoder(&Default::default());
        self.encode(&mut encoder, shape, dabs);
        if let Some(ts) = &self.timestamps {
            encoder.resolve_query_set(&ts.query_set, 0..2, &ts.resolve, 0);
            encoder.copy_buffer_to_buffer(&ts.resolve, 0, &ts.map, 0, 16);
        }
        self.queue.submit([encoder.finish()]);
        self.wait();
        let wall_ms = start.elapsed().as_secs_f64() * 1000.0;
        let gpu_ms = self.timestamps.as_ref().map(|ts| {
            let slice = ts.map.slice(..);
            slice.map_async(wgpu::MapMode::Read, |r| r.expect("map timestamps"));
            self.wait();
            let bytes = slice.get_mapped_range();
            let stamps: &[u64] = bytemuck::cast_slice(&bytes);
            let ns = stamps[1].saturating_sub(stamps[0]) as f64 * ts.period_ns as f64;
            drop(bytes);
            ts.map.unmap();
            ns / 1.0e6
        });
        Sample { wall_ms, gpu_ms }
    }

    fn encode(&self, encoder: &mut wgpu::CommandEncoder, shape: Shape, dabs: &[Dab]) {
        let n = dabs.len() as u32;
        match shape {
            Shape::DispatchRw | Shape::DispatchRw16 | Shape::DispatchRwRows | Shape::DispatchRo => {
                let timestamp_writes =
                    self.timestamps
                        .as_ref()
                        .map(|ts| wgpu::ComputePassTimestampWrites {
                            query_set: &ts.query_set,
                            beginning_of_pass_write_index: Some(0),
                            end_of_pass_write_index: Some(1),
                        });
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some(shape.label()),
                    timestamp_writes,
                });
                let (pipeline, bind_group) = match shape {
                    Shape::DispatchRw => (&self.compute_rw, &self.bind_group_rw),
                    Shape::DispatchRw16 => (&self.compute_rw_16, &self.bind_group_rw),
                    Shape::DispatchRwRows => (&self.compute_rw_rows, &self.bind_group_rw),
                    _ => (&self.compute_ro, &self.bind_group_ro),
                };
                pass.set_pipeline(pipeline);
                for (i, dab) in dabs.iter().enumerate() {
                    let groups = (2.0 * dab.radius / shape.tile() as f32).ceil() as u32 + 1;
                    pass.set_bind_group(0, bind_group, &[i as u32 * INDEX_STRIDE as u32]);
                    pass.dispatch_workgroups(groups, groups, 1);
                }
            }
            Shape::RenderPass => {
                for i in 0..n {
                    let timestamp_writes = self.timestamps.as_ref().and_then(|ts| {
                        let first = i == 0;
                        let last = i + 1 == n;
                        (first || last).then_some(wgpu::RenderPassTimestampWrites {
                            query_set: &ts.query_set,
                            beginning_of_pass_write_index: first.then_some(0),
                            end_of_pass_write_index: last.then_some(1),
                        })
                    });
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some(shape.label()),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &self.attachment_view,
                            resolve_target: None,
                            depth_slice: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Load,
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        timestamp_writes,
                        ..Default::default()
                    });
                    pass.set_pipeline(&self.render);
                    pass.set_bind_group(0, &self.bind_group_rw, &[i * INDEX_STRIDE as u32]);
                    pass.draw(0..6, 0..1);
                }
            }
        }
    }

    /// Run (a) and (c) once each from cleared targets and return the
    /// largest per-channel difference over the painted pixels, so the
    /// timed shapes are known to do equivalent work.
    fn parity(&self, dabs: &[Dab]) -> u8 {
        self.clear();
        self.queue
            .write_buffer(&self.records, 0, bytemuck::cast_slice(dabs));
        let mut encoder = self.device.create_command_encoder(&Default::default());
        self.encode(&mut encoder, Shape::DispatchRw, dabs);
        self.encode(&mut encoder, Shape::RenderPass, dabs);
        self.queue.submit([encoder.finish()]);
        self.wait();
        let packed = self.readback(&self.ground);
        let blended = self.readback(&self.attachment);
        let mut max_diff = 0u8;
        let (packed, _) = packed.as_chunks::<4>();
        let (blended, _) = blended.as_chunks::<4>();
        for (a, b) in packed.iter().zip(blended) {
            if a[3] == 0 && b[3] == 0 {
                continue;
            }
            for (x, y) in a.iter().zip(b) {
                max_diff = max_diff.max(x.abs_diff(*y));
            }
        }
        max_diff
    }

    /// Whole-texture readback of a 4-byte-per-texel texture. `r32uint`
    /// holds RGBA8 packed little-endian, so its bytes compare directly
    /// against `rgba8unorm`.
    fn readback(&self, texture: &wgpu::Texture) -> Vec<u8> {
        let bytes_per_row = TARGET_W * 4;
        assert_eq!(bytes_per_row % 256, 0, "row pitch must be 256-aligned");
        let size = (bytes_per_row * TARGET_H) as u64;
        let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("parity-readback"),
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &staging,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d {
                width: TARGET_W,
                height: TARGET_H,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        let slice = staging.slice(..);
        slice.map_async(wgpu::MapMode::Read, |r| r.expect("map readback"));
        self.wait();
        let data = slice.get_mapped_range().to_vec();
        staging.unmap();
        data
    }
}

fn git_sha() -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "--short=10", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into())
}

fn output_path() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    root.join("bench-results")
        .join(format!("dispatch-cost-bench-{}.md", git_sha()))
}
