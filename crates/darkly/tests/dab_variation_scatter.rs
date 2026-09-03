//! GPU coverage test for the Dab-space `variation` decorrelation offset.
//!
//! The bug this guards against: the per-dab offset was `vec2(v*64, v*64)`, the
//! same scalar on both axes, so every dab landed on the `x == y` diagonal of the
//! field (1D), and the `*64` stride resonated with the field's period. The fix
//! hashes `variation` into two independent components via `fbm_offset2`
//! (`shaders/lib/fbm2d.wgsl`), scattered over one field period.
//!
//! An emission-string unit test (`sample_frame.rs`) pins the *shape* of the
//! emitted WGSL but cannot execute `fbm_pcg`. This test executes the real
//! shipped `fbm_offset2` on the GPU for the exact input path a wired
//! `random → variation` produces, then asserts the results actually cover 2D and
//! are uncorrelated: the numerical property the bug was about.

use darkly::gpu::test_utils::test_device;

/// Number of per-dab samples to evaluate: the `random` node quantizes to full
/// f32 precision per dab; 1024 evenly-spaced draws is plenty to characterize the
/// distribution.
const N: u32 = 1024;
/// Grid resolution for the 2D-occupancy check.
const GRID: usize = 8;

/// Compute wrapper around the real, shipped `fbm2d.wgsl`. For each index it
/// reproduces the exact expression the emitter builds
/// (`fbm_offset2(u32(max(variation, 0.0) * 4096.0), period)`) with `period = 1.0`
/// so the two components land in `[0, 1)`, and the `variation` the standard
/// `random → variation` wire delivers (`random * 1024`, `random = i / N`).
const WRAPPER: &str = r#"
@group(0) @binding(0) var<storage, read_write> out_off: array<vec2<f32>>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if (i >= arrayLength(&out_off)) { return; }
    let random = f32(i) / f32(arrayLength(&out_off));
    let variation = random * 1024.0;              // wire-boundary remap
    let seed = u32(max(variation, 0.0) * 4096.0); // emitter's seed quantization
    out_off[i] = fbm_offset2(seed, 1.0);          // period 1.0 -> [0,1)^2
}
"#;

#[test]
fn dab_variation_offset_scatters_across_2d() {
    let (device, queue) = test_device();

    let mut source = String::from(include_str!("../shaders/lib/fbm2d.wgsl"));
    source.push_str(WRAPPER);
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("fbm-offset2-coverage"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });

    let bytes = (N as u64) * std::mem::size_of::<[f32; 2]>() as u64;
    let out_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("offsets"),
        size: bytes,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let read_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("offsets-read"),
        size: bytes,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("fbm-offset2-coverage"),
        layout: None,
        module: &module,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: out_buf.as_entire_binding(),
        }],
    });

    let mut encoder =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: None,
            timestamp_writes: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.dispatch_workgroups(N / 64, 1, 1);
    }
    encoder.copy_buffer_to_buffer(&out_buf, 0, &read_buf, 0, bytes);
    queue.submit([encoder.finish()]);

    // Blocking readback is test-only (native `poll(Wait)` drives the queue).
    let slice = read_buf.slice(..);
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    let _ = device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: None,
    });
    rx.recv().unwrap().expect("map failed");
    let data = slice.get_mapped_range();
    let offs: &[[f32; 2]] = bytemuck::cast_slice(&data);

    // 2D occupancy: bin into a GRID×GRID grid over [0,1)². A diagonal-only
    // distribution (the bug) fills at most GRID cells; genuine 2D scatter fills
    // the vast majority.
    let mut occupied = [[false; GRID]; GRID];
    for &[ox, oy] in offs {
        assert!(
            (0.0..1.0).contains(&ox) && (0.0..1.0).contains(&oy),
            "offset out of [0,1): ({ox}, {oy})"
        );
        let cx = ((ox * GRID as f32) as usize).min(GRID - 1);
        let cy = ((oy * GRID as f32) as usize).min(GRID - 1);
        occupied[cy][cx] = true;
    }
    let filled = occupied.iter().flatten().filter(|c| **c).count();
    assert!(
        filled >= 48,
        "expected ≥48/{} grid cells occupied (2D scatter); got {filled}: \
         the buggy diagonal fills ≤{GRID}",
        GRID * GRID,
    );

    // Low correlation: the diagonal has corr(ox, oy) ≈ 1.0; independent draws ≈ 0.
    let n = offs.len() as f64;
    let (mut sx, mut sy, mut sxx, mut syy, mut sxy) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for &[ox, oy] in offs {
        let (x, y) = (ox as f64, oy as f64);
        sx += x;
        sy += y;
        sxx += x * x;
        syy += y * y;
        sxy += x * y;
    }
    let cov = sxy / n - (sx / n) * (sy / n);
    let var_x = sxx / n - (sx / n).powi(2);
    let var_y = syy / n - (sy / n).powi(2);
    let corr = cov / (var_x.sqrt() * var_y.sqrt());
    assert!(
        corr.abs() < 0.15,
        "expected |corr(ox, oy)| < 0.15 (independent axes); got {corr:.3}",
    );

    drop(data);
    read_buf.unmap();
}
