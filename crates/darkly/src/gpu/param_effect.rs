//! The shared substrate for effects that are one fullscreen pass over a
//! parameter-derived [`EffectCache`].
//!
//! Most effects are the same object: hold a parameter vector, realize it into
//! GPU resources — a packed uniform, sometimes a baked texture (Curves and
//! Levels bake a 256×2 LUT) — then run one fragment pass reading `[src, …]` and
//! writing the destination. This is that object, so those effects are a
//! `register()` function and a description of their resources rather than a
//! hand-written [`Effect`] impl.
//!
//! An effect writes its own [`Effect`] implementation only when it needs
//! something this shape cannot express: multiple passes, intermediate render
//! targets, or animated state. `frozen`, `grain`, `lens_blur`, `painting`,
//! `pixelate`, `rainy_glass` and `vhs` are those.
//!
//! The bind-group shape is declared per effect as an ordered [`Binding`] list
//! matching the `@group(0) @binding(i)` numbering its shader uses, and
//! [`ParamEffect`] fills it in that order: the ping-pong half being read, then
//! the sampler if declared, then each aux view its resources produced, then
//! each uniform buffer.

use std::sync::Arc;

use crate::gpu::effect::{Binding, Effect, EffectCache, EffectPipeline};
use crate::gpu::params::{ParamDef, ParamValue};

/// Packs a parameter vector into the bytes of an effect's single uniform.
pub type PackUniform = fn(&[ParamValue]) -> Vec<u8>;

/// Allocates an effect's parameter-derived GPU resources into a fresh cache.
/// Runs once per instance, at cache creation — never on a parameter change.
pub type AllocResources = fn(&wgpu::Device, &mut EffectCache);

/// Writes current parameter values into resources [`AllocResources`] already
/// allocated. Runs at cache creation and again on every parameter change, so it
/// takes no device and allocates nothing. Boxed rather than a bare `fn` so a
/// shared substrate can close over the per-effect half of the work — which is
/// how Curves and Levels share one writer over two bakers.
pub type WriteResources = Box<dyn Fn(&wgpu::Queue, &[ParamValue], &EffectCache) + Send + Sync>;

/// What an effect's parameters realize into on the GPU.
///
/// The variants exist so that changing a parameter costs what it should:
/// [`Packed`](Resources::Packed) rewrites one buffer, [`Baked`](Resources::Baked)
/// re-fills resources it already owns, and neither reallocates or invalidates a
/// bind group. That is what lets [`Effect::set_params`] always answer `true`
/// here, so dragging a slider does not rebuild the instance.
pub enum Resources {
    /// Nothing to realize — the bind group is `[src]` alone.
    None,
    /// One uniform buffer, sized by what `pack` produces and rewritten from it
    /// on every parameter change.
    Packed(PackUniform),
    /// Anything else: `alloc` creates the buffers and textures once, `write`
    /// fills them from the current parameters.
    Baked {
        alloc: AllocResources,
        write: WriteResources,
    },
}

/// Everything shared by every instance of one [`ParamEffect`] type. Held behind
/// an `Arc` on each instance, so a hundred layers of the same effect share one.
pub struct ParamEffectKind {
    type_id: &'static str,
    label: &'static str,
    schema: &'static [ParamDef],
    bindings: &'static [Binding],
    resources: Resources,
}

impl std::fmt::Debug for ParamEffectKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParamEffectKind")
            .field("type_id", &self.type_id)
            .finish_non_exhaustive()
    }
}

impl ParamEffectKind {
    pub fn new(
        type_id: &'static str,
        label: &'static str,
        schema: &'static [ParamDef],
        bindings: &'static [Binding],
        resources: Resources,
    ) -> Arc<Self> {
        Arc::new(ParamEffectKind {
            type_id,
            label,
            schema,
            bindings,
            resources,
        })
    }

    /// Build an instance over `params`, padded to the full schema with defaults
    /// so [`Effect::param_values`] round-trips every slot.
    pub fn instance(
        self: &Arc<Self>,
        params: &[ParamValue],
        shared: Arc<EffectPipeline>,
    ) -> Box<dyn Effect> {
        Box::new(ParamEffect {
            params: pad_to_schema(self.schema, params),
            kind: self.clone(),
            shared,
        })
    }
}

/// Fill a partial parameter vector out to a schema, so every slot has a value
/// whatever the caller supplied.
fn pad_to_schema(schema: &'static [ParamDef], params: &[ParamValue]) -> Vec<ParamValue> {
    schema
        .iter()
        .enumerate()
        .map(|(i, def)| {
            params
                .get(i)
                .cloned()
                .unwrap_or_else(|| def.default_value())
        })
        .collect()
}

/// One instance of a [`ParamEffectKind`]: its own parameter values over the
/// shared kind and pipeline.
#[derive(Debug)]
pub struct ParamEffect {
    kind: Arc<ParamEffectKind>,
    params: Vec<ParamValue>,
    shared: Arc<EffectPipeline>,
}

impl ParamEffect {
    /// Write the current parameters into whatever resources `cache` holds. The
    /// one place [`Resources`] is interpreted for a write, so cache creation
    /// and a later parameter change cannot disagree.
    fn write_resources(&self, queue: &wgpu::Queue, cache: &EffectCache) {
        match &self.kind.resources {
            Resources::None => {}
            Resources::Packed(pack) => cache.write_uniform(queue, 0, &pack(&self.params)),
            Resources::Baked { write, .. } => write(queue, &self.params, cache),
        }
    }

    /// Build the two bind groups — one per ping-pong direction — over the
    /// resources `cache` holds. The one place the binding order is spelled out,
    /// so the cache and the shader cannot disagree.
    fn bind_groups(
        &self,
        device: &wgpu::Device,
        cache: &EffectCache,
        views: &[wgpu::TextureView; 2],
        sampler: &wgpu::Sampler,
    ) -> [wgpu::BindGroup; 2] {
        let wants_sampler = self
            .kind
            .bindings
            .iter()
            .any(|b| matches!(b, Binding::Sampler));
        std::array::from_fn(|i| {
            let mut entries = vec![wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&views[i]),
            }];
            let mut next = 1u32;
            if wants_sampler {
                entries.push(wgpu::BindGroupEntry {
                    binding: next,
                    resource: wgpu::BindingResource::Sampler(sampler),
                });
                next += 1;
            }
            for view in &cache.aux_views {
                entries.push(wgpu::BindGroupEntry {
                    binding: next,
                    resource: wgpu::BindingResource::TextureView(view),
                });
                next += 1;
            }
            for buf in &cache.uniform_bufs {
                entries.push(wgpu::BindGroupEntry {
                    binding: next,
                    resource: buf.as_entire_binding(),
                });
                next += 1;
            }
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&format!("{}-bg-{i}", self.kind.label)),
                layout: &self.shared.bind_group_layout,
                entries: &entries,
            })
        })
    }
}

impl Effect for ParamEffect {
    fn type_id(&self) -> &'static str {
        self.kind.type_id
    }

    fn clone_boxed(&self) -> Box<dyn Effect> {
        Box::new(ParamEffect {
            kind: self.kind.clone(),
            params: self.params.clone(),
            shared: self.shared.clone(),
        })
    }

    fn param_values(&self) -> Vec<ParamValue> {
        self.params.clone()
    }

    fn create_cache(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        ping_pong_views: &[wgpu::TextureView; 2],
        sampler: &wgpu::Sampler,
        _render_width: u32,
        _render_height: u32,
    ) -> EffectCache {
        let mut cache = EffectCache::empty();
        match &self.kind.resources {
            Resources::None => {}
            Resources::Packed(pack) => {
                let bytes = pack(&self.params);
                cache
                    .uniform_bufs
                    .push(device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some(&format!("{}-uniform", self.kind.label)),
                        size: bytes.len() as u64,
                        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    }));
            }
            Resources::Baked { alloc, .. } => alloc(device, &mut cache),
        }
        self.write_resources(queue, &cache);
        cache.bind_groups = vec![self.bind_groups(device, &cache, ping_pong_views, sampler)];
        cache
    }

    fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        cache: &EffectCache,
        src_idx: usize,
        dst_view: &wgpu::TextureView,
    ) {
        let Some(groups) = cache.bind_groups.first() else {
            return;
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(self.kind.label),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: dst_view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&self.shared.pipeline);
        pass.set_bind_group(0, &groups[src_idx], &[]);
        pass.draw(0..3, 0..1);
    }

    /// Always `true`: every [`Resources`] variant refreshes in place, so the
    /// bind groups built at cache creation still describe this instance. This
    /// is what makes a slider drag cost one buffer write rather than a rebuild.
    fn set_params(
        &mut self,
        queue: &wgpu::Queue,
        cache: &EffectCache,
        params: &[ParamValue],
    ) -> bool {
        self.params = pad_to_schema(self.kind.schema, params);
        self.write_resources(queue, cache);
        true
    }
}
