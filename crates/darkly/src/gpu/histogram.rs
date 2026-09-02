//! GPU-accelerated per-channel histogram.
//!
//! A compute shader bins a texture into eight 256-bin histograms (one per LUT
//! filter channel — rgb-composite, red, green, blue, alpha, hue, saturation,
//! lightness) using atomic adds, then reads back the 8×256 u32 result
//! asynchronously — no blocking readback. Composite and Lightness both bin
//! CIELAB L*, R/G/B/A bin the raw gamma-encoded value (matching Krita's
//! `KoBasicHistogramProducers`). Modeled on
//! [`ContentBoundsPass`](super::content_bounds).
//!
//! Unlike content bounds (which reads a node's own persistent texture and can
//! self-submit), the histogram samples a filter's *input* — a group accumulator
//! that is only valid mid-composite — so [`dispatch`] records into the caller's
//! in-flight compose encoder rather than owning one.
//!
//! [`dispatch`]: HistogramPass::dispatch

use super::revisions::{Revisions, Tick};
use crate::layer::LayerId;
use std::collections::HashMap;

/// Number of virtual channels (matches the LUT filter's channel set).
pub const HIST_CHANNELS: usize = 8;
/// Bins per channel (one per 8-bit value).
pub const HIST_BINS: usize = 256;
/// Total u32 entries in the 8×256 buffer.
const HIST_LEN: usize = HIST_CHANNELS * HIST_BINS;
/// Byte length of the histogram buffer.
const HIST_BYTES: u64 = (HIST_LEN * 4) as u64;

/// GPU compute pipeline + per-layer histogram cache.
pub struct HistogramPass {
    pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,

    /// Cached 8×256 histogram per layer, channel-major, with the pixel
    /// revision it was binned under.
    cached: HashMap<LayerId, (Tick, Vec<u32>)>,
    /// In-flight dispatches awaiting buffer mapping.
    pending: Vec<PendingHistogram>,
}

/// The revision a histogram is valid against: any node's pixel write.
///
/// Deliberately the aggregate rather than the target's own tick — a filter's
/// histogram bins its *input* accumulator, which is every node beneath it, and
/// deliberately not the document, so a Levels drag does not discard the
/// histogram it is being read against.
fn stamp(revisions: &Revisions) -> Tick {
    revisions.node_pixels_any()
}

struct PendingHistogram {
    layer_id: LayerId,
    stamp: Tick,
    staging: wgpu::Buffer,
    rx: Option<std::sync::mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>>,
}

/// Uniform buffer layout matching the shader's `Params` (padded to 16 bytes).
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    width: u32,
    height: u32,
    _pad0: u32,
    _pad1: u32,
}

impl HistogramPass {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("histogram-shader"),
            source: wgpu::ShaderSource::Wgsl(
                format!(
                    "{}\n{}",
                    include_str!("../../shaders/lib/colorspace.wgsl"),
                    include_str!("../../shaders/histogram.wgsl"),
                )
                .into(),
            ),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("histogram-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("histogram-pipeline-layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("histogram-pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });

        HistogramPass {
            pipeline,
            bind_group_layout,
            cached: HashMap::new(),
            pending: Vec::new(),
        }
    }

    /// The current 8×256 histogram (channel-major) for a layer, if one was
    /// binned under the live pixel revision.
    pub fn get(&self, revisions: &Revisions, layer_id: LayerId) -> Option<&[u32]> {
        let (t, hist) = self.cached.get(&layer_id)?;
        (*t == stamp(revisions)).then_some(hist.as_slice())
    }

    /// True if a current histogram result is cached for this layer.
    pub fn has_cached(&self, revisions: &Revisions, layer_id: LayerId) -> bool {
        self.get(revisions, layer_id).is_some()
    }

    /// True if a dispatch against the live pixel revision is in flight.
    pub fn is_pending(&self, revisions: &Revisions, layer_id: LayerId) -> bool {
        let stamp = stamp(revisions);
        self.pending
            .iter()
            .any(|p| p.layer_id == layer_id && p.stamp == stamp)
    }

    /// True when a fresh dispatch is warranted: nothing current cached and
    /// nothing current in flight.
    pub fn needs(&self, revisions: &Revisions, layer_id: LayerId) -> bool {
        !self.has_cached(revisions, layer_id) && !self.is_pending(revisions, layer_id)
    }

    /// True if any results are pending.
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Drop all state for a layer (when it's deleted or unfocused).
    pub fn remove_layer(&mut self, layer_id: LayerId) {
        self.cached.remove(&layer_id);
    }

    /// Record a histogram compute + staging copy into `encoder`, sampling
    /// `texture_view` (the filter's input accumulator). The result lands after
    /// the encoder is submitted; retrieve via [`poll`] + [`get`].
    ///
    /// [`poll`]: HistogramPass::poll
    /// [`get`]: HistogramPass::get
    #[allow(clippy::too_many_arguments)]
    pub fn dispatch(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        revisions: &Revisions,
        texture_view: &wgpu::TextureView,
        width: u32,
        height: u32,
        layer_id: LayerId,
    ) {
        if width == 0 || height == 0 {
            return;
        }
        let stamp = stamp(revisions);
        if self
            .pending
            .iter()
            .any(|p| p.layer_id == layer_id && p.stamp == stamp)
        {
            return;
        }

        // Zero-initialized storage buffer for the atomic bins.
        let storage_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("histogram-storage"),
            size: HIST_BYTES,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: true,
        });
        {
            let mut mapping = storage_buf.slice(..).get_mapped_range_mut();
            mapping.copy_from_slice(&[0u8; HIST_LEN * 4]);
        }
        storage_buf.unmap();

        let staging_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("histogram-staging"),
            size: HIST_BYTES,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let params = Params {
            width,
            height,
            _pad0: 0,
            _pad1: 0,
        };
        let param_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("histogram-params"),
            size: std::mem::size_of::<Params>() as u64,
            usage: wgpu::BufferUsages::UNIFORM,
            mapped_at_creation: true,
        });
        {
            let mut mapping = param_buf.slice(..).get_mapped_range_mut();
            mapping.copy_from_slice(bytemuck::bytes_of(&params));
        }
        param_buf.unmap();

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("histogram-bg"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(texture_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: storage_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: param_buf.as_entire_binding(),
                },
            ],
        });

        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("histogram"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, Some(&bind_group), &[]);
            pass.dispatch_workgroups(width.div_ceil(16), height.div_ceil(16), 1);
        }
        encoder.copy_buffer_to_buffer(&storage_buf, 0, &staging_buf, 0, HIST_BYTES);

        self.pending.push(PendingHistogram {
            layer_id,
            stamp,
            staging: staging_buf,
            rx: None,
        });
    }

    /// Poll pending computations. Call once per frame after the compose submit.
    /// Returns the layer IDs whose histograms just became available.
    pub fn poll(&mut self, device: &wgpu::Device, revisions: &Revisions) -> Vec<LayerId> {
        for p in &mut self.pending {
            if p.rx.is_none() {
                let slice = p.staging.slice(..);
                let (tx, rx) = std::sync::mpsc::sync_channel(1);
                slice.map_async(wgpu::MapMode::Read, move |result| {
                    let _ = tx.send(result);
                });
                p.rx = Some(rx);
            }
        }

        if !self.pending.is_empty() {
            let _ = device.poll(wgpu::PollType::Poll);
        }

        let mut completed = Vec::new();
        let mut i = 0;
        while i < self.pending.len() {
            let ready = self.pending[i]
                .rx
                .as_ref()
                .and_then(|rx| rx.try_recv().ok());
            match ready {
                Some(Ok(())) => {
                    let p = self.pending.swap_remove(i);
                    if p.stamp == stamp(revisions) {
                        let slice = p.staging.slice(..);
                        let mapped = slice.get_mapped_range();
                        let bins: Vec<u32> =
                            bytemuck::cast_slice::<u8, u32>(&mapped[..HIST_BYTES as usize])
                                .to_vec();
                        drop(mapped);
                        p.staging.unmap();
                        self.cached.insert(p.layer_id, (p.stamp, bins));
                        completed.push(p.layer_id);
                    } else {
                        p.staging.unmap();
                    }
                }
                Some(Err(e)) => {
                    log::error!("histogram buffer mapping failed: {e}");
                    self.pending.swap_remove(i);
                }
                None => i += 1,
            }
        }
        completed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::test_utils::{create_test_texture, test_device};

    /// A constant-color texture lands all its pixels in the expected per-channel
    /// bins. Red/Green/Blue/Alpha bins are exact (8-bit value → bin index); the
    /// derived channels (composite/hue/sat/lightness) are checked by total count.
    #[test]
    fn constant_color_bins_each_channel() {
        let (device, queue) = test_device();
        let mut pass = HistogramPass::new(&device);

        // 4×4, every pixel (r=128, g=64, b=200, a=255).
        let (w, h) = (4u32, 4u32);
        let color = [128u8, 64, 200, 255];
        let pixels: Vec<u8> = color
            .iter()
            .copied()
            .cycle()
            .take((w * h * 4) as usize)
            .collect();
        let (_tex, view) = create_test_texture(&device, &queue, w, h, &pixels);
        let layer = LayerId::from_ffi(1);

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("histogram-test"),
        });
        let revisions = Revisions::new();
        pass.dispatch(&device, &mut encoder, &revisions, &view, w, h, layer);
        queue.submit([encoder.finish()]);
        let _ = device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });

        let mut done = Vec::new();
        for _ in 0..100 {
            done = pass.poll(&device, &revisions);
            if !done.is_empty() {
                break;
            }
            let _ = device.poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            });
        }
        assert!(done.contains(&layer), "histogram result never landed");

        let bins = pass.get(&revisions, layer).expect("cached histogram");
        let total = w * h;
        let channel = |c: usize| &bins[c * HIST_BINS..(c + 1) * HIST_BINS];

        // Every channel accounts for all 16 pixels.
        for c in 0..HIST_CHANNELS {
            assert_eq!(
                channel(c).iter().sum::<u32>(),
                total,
                "channel {c} total bin count"
            );
        }
        // Exact bins for the raw color channels (order: luma,r,g,b,a,...).
        assert_eq!(channel(1)[128], total, "red bin 128");
        assert_eq!(channel(2)[64], total, "green bin 64");
        assert_eq!(channel(3)[200], total, "blue bin 200");
        assert_eq!(channel(4)[255], total, "alpha bin 255");
    }
}
