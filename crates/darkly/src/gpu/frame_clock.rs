//! The [`Compositor`]'s frame scheduler: the master rAF tick counter and the
//! divisor-throttled animation ticks for animated voids, effect layers on
//! both sides of the screen-space divider, and the tool overlay.

use crate::document::Document;
use crate::gpu::compositor::Compositor;
use crate::gpu::effect_layers::{EffectInstance, EffectSpace};
use crate::layer::LayerId;
use smallvec::SmallVec;

/// The nodes one animation tick advanced. Documents animate a handful of
/// layers at once; wider sets spill to the heap without ceremony.
type TickedNodes = SmallVec<[LayerId; 8]>;

impl Compositor {
    /// Master rAF tick counter. Advances exactly once per `update_animations`
    /// call (i.e. once per `engine.render`), starting at 0. This is the same
    /// counter every divisor-throttled subsystem inside the compositor checks
    /// (`screen_divisor`, `overlay_divisor`, `canvas_divisor` — see
    /// [`Self::update_animations`]), so any JS-side throttle that uses
    /// `frame_count % divisor == 0` automatically aligns with all of them.
    /// Exposed so the WASM bridge can hand it to the frontend (e.g. the
    /// camera void's upload throttle).
    pub fn frame_count(&self) -> u64 {
        self.frame_count
    }

    /// Unified frame scheduler. Called once per rAF tick.
    ///
    /// Systems fire at fractional rates of the master clock (rAF rate):
    /// - Viewport-only effects: every `screen_divisor`-th frame (default 2 =
    ///   50% = 30fps at 60hz)
    /// - Overlay: every `overlay_divisor`-th frame (default 4 = 25% = 15fps at 60hz)
    /// - Document content — void layers and canvas-space effect layers: every
    ///   `canvas_divisor`-th frame
    ///
    /// Integer divisors guarantee alignment — a divisor-4 tick always coincides
    /// with a divisor-2 tick, so systems never force extra frame renders.
    ///
    /// `doc` is borrowed to consult layer visibility — animation work for an
    /// effectively-hidden layer (self or any ancestor hidden) is skipped at
    /// exactly the point the compositor's tree walk would drop the layer's
    /// composited output.
    pub fn update_animations(&mut self, queue: &wgpu::Queue, wall_time: f32, doc: &Document) {
        let dt = if self.last_wall_time > 0.0 {
            (wall_time - self.last_wall_time).max(0.0)
        } else {
            0.0
        };
        self.last_wall_time = wall_time;
        self.frame_count += 1;

        if dt == 0.0 {
            return;
        }

        let screen_divisor = crate::config::get_i64("animation.screen_divisor") as u64;
        let overlay_divisor = crate::config::get_i64("animation.overlay_divisor") as u64;
        let canvas_divisor = crate::config::get_i64("animation.canvas_divisor") as u64;

        let screen_fires = screen_divisor > 0
            && self.any_animated_effect(doc, true)
            && self.frame_count.is_multiple_of(screen_divisor);

        let overlay_fires = overlay_divisor > 0
            && self.tool_overlay.needs_animation()
            && self.frame_count.is_multiple_of(overlay_divisor);

        // Each animated subsystem advances on its own integer divisor of
        // the master rAF clock; integer divisors guarantee no subsystem
        // forces a frame another subsystem wouldn't already produce. See
        // `docs/lessons-learned/gpu-lessons-learned.md` master-clock
        // principle.
        let canvas_fires = canvas_divisor > 0
            && (self.any_animated_layer(doc) || self.any_animated_effect(doc, false))
            && self.frame_count.is_multiple_of(canvas_divisor);

        if screen_fires {
            self.tick_animated_effects(queue, dt * screen_divisor as f32, doc, true);
        }

        if overlay_fires {
            self.tool_overlay.advance_time(dt * overlay_divisor as f32);
        }

        if canvas_fires {
            // This side of the divider is document content, so a tick feeds
            // the composite rather than the presented frame. Each advanced
            // node records its own bump: the walk needs to know *which*
            // subtree moved to reuse the rest, and that is exactly what the
            // loops below already know.
            let mut ticked = self.tick_animated_layers(queue, dt * canvas_divisor as f32, doc);
            ticked.extend(self.tick_animated_effects(
                queue,
                dt * canvas_divisor as f32,
                doc,
                false,
            ));
            for id in ticked {
                self.revisions.bump_animation(id);
            }
        }

        if screen_fires || overlay_fires {
            self.revisions.bump_present_inputs();
        }
    }

    /// Returns true if any animations need continuous frames (effect layers on
    /// either side of the divider, the overlay, or any effectively-visible
    /// animated layer). `doc` is consulted for per-layer visibility — same
    /// contract as [`Self::update_animations`].
    pub fn needs_animation(&self, doc: &Document) -> bool {
        self.tool_overlay.needs_animation()
            || self.any_animated_effect(doc, true)
            || self.any_animated_effect(doc, false)
            || self.any_animated_layer(doc)
    }

    /// True when any allocated layer with procedural content reports
    /// `needs_animation()` AND is effectively visible in `doc`. Folded into
    /// the compositor's overall `needs_animation()` so the rAF loop keeps
    /// ticking while animated voids exist — but a hidden layer's animation
    /// contribution is dropped at the same point the compositor's tree walk
    /// would drop the layer's output (see `compose_children`'s
    /// `node.visible()` skip).
    fn any_animated_layer(&self, doc: &Document) -> bool {
        self.procedural_entries()
            .any(|(id, p)| p.void.needs_animation() && doc.effective_visible(id))
    }

    /// Whether an effect instance takes part in an animation tick for one side
    /// of the divider: it is realized in that space, it animates at its current
    /// parameters, and it is effectively visible. The scheduler's predicate and
    /// the tick itself both go through here, so they cannot drift apart about
    /// which instances are in scope.
    fn effect_animates(inst: &EffectInstance, id: LayerId, doc: &Document, screen: bool) -> bool {
        (inst.space == EffectSpace::Screen) == screen
            && inst.effect.needs_animation()
            && doc.effective_visible(id)
    }

    /// Whether any effect layer on one side of the divider wants continuous
    /// frames. The instance is the authority — `needs_animation()` is an answer
    /// about current parameter values, which only the realized effect holds.
    fn any_animated_effect(&self, doc: &Document, screen: bool) -> bool {
        self.effect_instances
            .iter()
            .any(|(id, inst)| Self::effect_animates(inst, *id, doc, screen))
    }

    /// Advance every effectively-visible animated effect instance in one space
    /// by `dt`, returning the ids that advanced. Which space is a parameter
    /// rather than two loops because the instances are one map and the only
    /// difference is what the caller records afterwards.
    fn tick_animated_effects(
        &mut self,
        queue: &wgpu::Queue,
        dt: f32,
        doc: &Document,
        screen: bool,
    ) -> TickedNodes {
        let mut ticked = TickedNodes::new();
        for (id, inst) in self.effect_instances.iter_mut() {
            if !Self::effect_animates(inst, *id, doc, screen) {
                continue;
            }
            inst.effect.update_time(queue, &inst.cache, dt);
            ticked.push(*id);
        }
        ticked
    }

    /// Advance every effectively-visible animated layer's procedural
    /// content by `dt`, returning the ids that advanced. Called by
    /// `update_animations` at the cadence set by `animation.canvas_divisor`.
    /// Visibility is queried the same way the main composite walk queries
    /// it — no precomputed "hidden" set; the doc is the authoritative tree.
    fn tick_animated_layers(
        &mut self,
        queue: &wgpu::Queue,
        dt: f32,
        doc: &Document,
    ) -> TickedNodes {
        let mut ticked = TickedNodes::new();
        for (id, proc) in self.procedural_entries_mut() {
            if !proc.void.needs_animation() || !doc.effective_visible(id) {
                continue;
            }
            proc.void.update_time(queue, &proc.cache, dt);
            ticked.push(id);
        }
        ticked
    }
}
