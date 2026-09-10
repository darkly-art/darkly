//! Effect-layer realization on [`Compositor`]: the per-layer
//! [`EffectInstance`] state for both canvas-space and screen-space effects,
//! the pre-compose sync that rebuilds instances when their inputs drift, and
//! the apply-pass uniforms they carry.

use crate::document::Document;
use crate::gpu::compositor::{ApplyUniforms, Compositor};
use crate::gpu::create_uniform_buffer;
use crate::gpu::params::ParamValue;
use crate::gpu::revisions::Tick;
use crate::layer::LayerId;
use std::collections::HashSet;

/// One realized effect layer: the instance, its resolution scaffolding, the
/// cache it built, and the facts it was built against.
///
/// Every field below the cache is a fingerprint. `sync_effect_instances`
/// compares them against the document and the compositor's current textures,
/// and rebuilds on any drift, which is what makes the compose walk a pure
/// encode with nothing to check.
/// The pair an effect instance was prepared against. An effect layer is the
/// same object in both spaces (one shader, one param schema) but the textures
/// it binds, the resolution it runs at and the dirty flag it drives all follow
/// from which side of the divider it sits on.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum EffectSpace {
    /// Inside the tree walk, writing into this group's accumulator.
    Canvas { parent: LayerId },
    /// After the present pass, on the view-transformed image.
    Screen,
}

pub(super) struct EffectInstance {
    pub(super) effect: Box<dyn crate::gpu::effect::Effect>,
    pub(super) scaled: crate::gpu::effect_scaling::ScaledEffect,
    pub(super) cache: crate::gpu::effect::EffectCache,
    /// Parameter values this instance currently holds.
    params: Vec<ParamValue>,
    /// Effect type this instance was built from.
    pipeline_id: String,
    /// Which space this instance was realized in, and therefore which
    /// ping-pong pair its bind groups point at.
    pub(super) space: EffectSpace,
    /// Dimensions of that pair.
    render_size: (u32, u32),
    /// The `targets` revision the bind groups were built under. Any bump
    /// means the textures behind them may have been freed.
    built_targets: Tick,
    /// The effective scale this instance's scaffolding was built for. The
    /// instance is the only record of the scale in force (nothing caches a
    /// second copy) so a configuration change is detected by comparing against
    /// this rather than by watching the config from somewhere else.
    applied_scale: f32,
    /// This layer's uniform for the shared in-place apply pass. Per instance
    /// rather than one reused buffer, because several effect layers encode into
    /// the same command encoder and a shared buffer would hand every one of
    /// them the last write.
    pub(super) apply_uniform: wgpu::Buffer,
}

impl Compositor {
    /// Cumulative count of from-scratch effect-instance builds. A caching
    /// regression shows up here as a number that tracks the frame count.
    #[cfg(any(test, feature = "testing"))]
    pub fn effect_rebuilds(&self) -> u64 {
        self.effect_rebuilds
    }

    /// Wake the pipeline when the configured effect scale has drifted from what
    /// the realized instances were built at. The instances are the record of
    /// the scale in force, so nothing here caches a second copy;
    /// `sync_effect_instances` does the rebuilding once a frame is running.
    ///
    /// Only instances that sync can actually reach are consulted. One whose
    /// space currently has no resources (a zero-sized viewport drops the screen
    /// run's textures while its entries survive) would never be rebuilt, and
    /// counting it as drifted would mark the compositor dirty on every frame
    /// forever. Skipping it is what makes this terminate.
    ///
    /// Called from both frame entry points, since each gates on a dirty flag the
    /// other does not reach; polling twice in one frame is harmless, because the
    /// second call sees zero drift.
    pub(super) fn sync_effect_scale(&mut self) {
        let base = crate::gpu::effect_scaling::effect_scale();
        let drifted = self.effect_instances.iter().any(|(_, inst)| {
            let reachable = match inst.space {
                EffectSpace::Canvas { parent } => self.group_state.contains_key(&parent),
                EffectSpace::Screen => self.screen_run.views().is_some(),
            };
            reachable
                && (inst.applied_scale
                    - crate::gpu::effect_scaling::effective_scale(
                        base,
                        inst.effect.perf_scale_factor(),
                    ))
                .abs()
                    >= crate::gpu::effect_scaling::SCALE_EPSILON
        });
        if drifted {
            self.revisions.bump_document();
            self.revisions.bump_present_inputs();
        }
    }

    /// The resolution an effect layer's realized instance renders at, or `None`
    /// when it runs at full scale on its space's own pair. `None` also covers
    /// "no instance realized yet".
    #[cfg(any(test, feature = "testing"))]
    pub fn effect_reduced_size(&self, id: LayerId) -> Option<(u32, u32)> {
        self.effect_instances.get(&id)?.scaled.reduced_size()
    }

    /// (Re-)allocate the canvas-space apply scratch to match the accumulators.
    ///
    /// Sized like a `GroupState`'s accumulator because it stands in for one:
    /// the effect writes here instead of into the other ping-pong half, so the
    /// apply pass can read both halves and still have a destination.
    fn ensure_canvas_apply_scratch(&mut self, device: &wgpu::Device) {
        let (w, h) = (self.canvas_width, self.canvas_height);
        if w == 0 || h == 0 {
            return;
        }
        let matches = self
            .canvas_apply_scratch
            .as_ref()
            .is_some_and(|(t, _)| t.width() == w && t.height() == h);
        if matches {
            return;
        }
        self.canvas_apply_scratch = Some(Self::make_accum_texture(
            device,
            w,
            h,
            "canvas-effect-apply-scratch",
        ));
        self.revisions.bump_targets();
    }

    /// Build one in-place host's apply uniform: the canvas window, the host
    /// mask's own plane rect, and the modulation the host contributes.
    ///
    /// The mask geometry is what lets a mask that grew independently of the
    /// canvas window sample in its own space: the same `sample_mask_window`
    /// path the leaf projection takes. A mask being transform-previewed is
    /// canvas-aligned; otherwise it samples in its live extent. A host with no
    /// visible mask gets the canvas rect, against which the shader's fallback
    /// white mask reads as fully covered.
    pub(super) fn apply_uniforms_for(
        &self,
        doc: &Document,
        host_id: LayerId,
        blend_mode: u32,
        opacity: f32,
        canvas_size: [f32; 2],
        isolated: Option<LayerId>,
    ) -> ApplyUniforms {
        let canvas_origin = [self.canvas_origin.x as f32, self.canvas_origin.y as f32];
        let mask_id = doc.visible_mask_of(host_id);
        let (mask_offset, mask_size) = match mask_id {
            Some(id) if self.transform_preview_target(id).is_none() => self
                .node_textures
                .get(&id)
                .map(|s| s.texture.canvas_extent())
                .map(|e| {
                    (
                        [e.x0() as f32, e.y0() as f32],
                        [e.width as f32, e.height as f32],
                    )
                })
                .unwrap_or((canvas_origin, canvas_size)),
            _ => (canvas_origin, canvas_size),
        };
        ApplyUniforms {
            canvas_origin,
            canvas_size,
            mask_offset,
            mask_size,
            isolated: (mask_id.is_some() && isolated == mask_id) as u32,
            blend_mode,
            opacity,
            _pad0: 0,
        }
    }

    /// Bring every effect layer's realized instance up to date with the
    /// document, then discard the ones whose layers are gone.
    ///
    /// The one place with both a `device` and a `queue` on the effect path:
    /// the compose walk that follows only *encodes*, so everything an encode
    /// could need must already exist when this returns. That is why an instance
    /// records what it was built against: this compares those facts and
    /// rebuilds on any drift, rather than the walk re-deriving them per frame.
    ///
    /// Rebuilding is the expensive branch and it is avoided wherever an effect
    /// can adopt the change in place: `Effect::set_params` answering `true`
    /// means a slider drag costs one buffer write. Everything else (a different
    /// effect type, a different parent, a resized accumulator, a freed texture)
    /// genuinely needs new bind groups.
    pub(super) fn sync_effect_instances(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        doc: &Document,
        isolated: Option<LayerId>,
    ) {
        self.ensure_canvas_apply_scratch(device);
        self.screen_run.ensure_resources(device);

        // One pass over the document's effect layers, each tagged with the
        // space its position puts it in. Everything downstream (the pair it
        // binds, the scale it runs at, the dirty flag it drives) follows from
        // this tag, so there is no second list to keep in step.
        // Flattened, so an effect nested in a run group is tagged by the space
        // it actually renders in rather than by whether it is a root child.
        let screen_run: HashSet<LayerId> = doc.screen_space_effects().into_iter().collect();
        let live: Vec<(LayerId, EffectSpace, String, Vec<ParamValue>)> = doc
            .all_filter_layers()
            .iter()
            .filter_map(|f| {
                let space = if screen_run.contains(&f.id) {
                    EffectSpace::Screen
                } else {
                    EffectSpace::Canvas {
                        parent: doc.accumulator_host_of(f.id)?,
                    }
                };
                Some((f.id, space, f.pipeline.clone(), f.params.clone()))
            })
            .collect();

        let ids: HashSet<LayerId> = live.iter().map(|(id, ..)| *id).collect();
        self.effect_instances.retain(|id, _| ids.contains(id));

        let scale = crate::gpu::effect_scaling::effect_scale();

        for (id, space, pipeline_id, params) in live {
            // The native size the instance renders against. The scale it runs
            // under is global (one knob for both spaces) so only the pair's
            // dimensions differ here.
            let size = match space {
                EffectSpace::Canvas { parent } => {
                    let Some(gs) = self.group_state.get(&parent) else {
                        // The parent's accumulator does not exist yet; the next
                        // frame that creates it bumps the `targets` revision
                        // and we build then.
                        continue;
                    };
                    (gs.accum.textures[0].width(), gs.accum.textures[0].height())
                }
                EffectSpace::Screen => {
                    if self.screen_run.views().is_none() {
                        // No viewport size yet.
                        continue;
                    }
                    self.screen_run.viewport_size()
                }
            };
            if size.0 == 0 || size.1 == 0 {
                continue;
            }

            // Everything except the parameters is structural: a change means
            // the bind groups no longer describe reality. The scale belongs
            // here because it sizes the scaffolding without moving
            // `render_size`, which is the native pair either way.
            let structural_match = self.effect_instances.get(&id).is_some_and(|inst| {
                inst.pipeline_id == pipeline_id
                    && inst.space == space
                    && inst.render_size == size
                    && inst.built_targets == self.revisions.targets()
                    && (inst.applied_scale
                        - crate::gpu::effect_scaling::effective_scale(
                            scale,
                            inst.effect.perf_scale_factor(),
                        ))
                    .abs()
                        < crate::gpu::effect_scaling::SCALE_EPSILON
            });

            if structural_match {
                let inst = self.effect_instances.get_mut(&id).expect("matched above");
                if inst.params == params {
                    continue;
                }
                if inst.effect.set_params(queue, &inst.cache, &params) {
                    inst.params = params;
                    continue;
                }
                // The effect cannot adopt these in place: fall through and
                // rebuild it against the same views.
            }

            self.effect_rebuilds += 1;
            // An instance of the same effect type is cloned rather than rebuilt
            // from the registry, so a rebuild triggered by resources moving
            // under it (a resize, a scale change) keeps whatever the effect
            // was carrying. Animation clocks live on the effect itself, so
            // going back to the registry would silently rewind every animated
            // veil to zero.
            // Same type and same parameters only: the fall-through from a
            // refused `set_params` needs a fresh instance built from the new
            // values, and a clone would carry the old ones into its cache.
            let reusable = self
                .effect_instances
                .get(&id)
                .filter(|inst| inst.pipeline_id == pipeline_id && inst.params == params)
                .map(|inst| inst.effect.clone_boxed());
            let Some(mut effect) = reusable.or_else(|| {
                self.effect_registry.instance(
                    &pipeline_id,
                    &params,
                    device,
                    wgpu::TextureFormat::Rgba8Unorm,
                )
            }) else {
                // An unknown effect id (a save naming one this binary does not
                // ship) simply has no instance, and composes as a no-op.
                self.effect_instances.remove(&id);
                continue;
            };

            // Borrowed here rather than above: the registry call needs
            // `&mut self`, which cannot coexist with a borrow of either pair.
            let (views, pipelines) = match space {
                EffectSpace::Canvas { parent } => (
                    &self.group_state[&parent].accum.views,
                    &self.canvas_scaling_pipelines,
                ),
                EffectSpace::Screen => {
                    let (Some(views), Some(pipelines)) =
                        (self.screen_run.views(), self.screen_run.scaling_pipelines())
                    else {
                        continue;
                    };
                    (views, pipelines)
                }
            };
            let effect_scale_factor = effect.perf_scale_factor();
            let (scaled, cache) = crate::gpu::effect_scaling::ScaledEffect::prepare(
                device,
                queue,
                &mut *effect,
                views,
                &self.sampler,
                pipelines,
                wgpu::TextureFormat::Rgba8Unorm,
                size.0,
                size.1,
                scale,
            );
            let apply_uniform =
                create_uniform_buffer::<ApplyUniforms>(device, "effect-apply-uniform");
            self.effect_instances.insert(
                id,
                EffectInstance {
                    effect,
                    scaled,
                    cache,
                    params,
                    pipeline_id,
                    space,
                    render_size: size,
                    built_targets: self.revisions.targets(),
                    applied_scale: crate::gpu::effect_scaling::effective_scale(
                        scale,
                        effect_scale_factor,
                    ),
                    apply_uniform,
                },
            );
        }

        // The apply uniform belongs to the instance, so it is written here
        // rather than by whichever caller happened to run the sync: a rebuild
        // triggered from the present path (a viewport resize never dirties the
        // composite) would otherwise leave a fresh buffer unwritten, and the
        // effect would silently compose at opacity zero.
        let canvas_size = [self.canvas_width as f32, self.canvas_height as f32];
        let uniforms: Vec<(wgpu::Buffer, ApplyUniforms)> = doc
            .all_filter_layers()
            .iter()
            .filter_map(|f| {
                let inst = self.effect_instances.get(&f.id)?;
                Some((
                    inst.apply_uniform.clone(),
                    self.apply_uniforms_for(
                        doc,
                        f.id,
                        f.blend.blend_mode.gpu_value,
                        f.blend.opacity,
                        canvas_size,
                        isolated,
                    ),
                ))
            })
            .collect();
        for (buf, u) in uniforms {
            queue.write_buffer(&buf, 0, bytemuck::bytes_of(&u));
        }
    }
}
