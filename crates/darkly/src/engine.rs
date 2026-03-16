use crate::clipboard::{Clipboard, ImageClip};
use crate::document::{Document, MoveTarget, SelectionMode};
use crate::gpu::transform::{FloatingContent, FloatingMode, Affine2D, IDENTITY, source_from_clip};
use crate::layer::{BlendMode, Layer, LayerNode};
use crate::undo::{
    UndoStack, GpuRegionAction, LayerAddAction, LayerRemoveAction, LayerMoveAction,
    MaskPropertyAction, PropertyAction, SelectionAction,
};
use crate::undo::property::Property;
use crate::gpu::compositor::Compositor;
use crate::gpu::context::GpuContext;
use crate::gpu::flood_fill;
use crate::gpu::overlay::{
    OverlayPrimitive, KIND_DASHED_LINE, FLAG_CANVAS_SPACE,
};
use crate::gpu::paint_target::{GpuPaintTarget, PaintPipelines};
use crate::gpu::params::{ParamDef, ParamValue};
use crate::gpu::readback::{self, ReadbackRequest};
use crate::gpu::region_store::RegionStore;
use crate::gpu::view::ViewTransform;
use crate::tile::{AlphaMask, TileGrid, TILE_SIZE};

// ---------------------------------------------------------------------------
// Shared return types — serde-serializable for any FFI bridge.
// ---------------------------------------------------------------------------

#[derive(serde::Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum LayerInfo {
    #[serde(rename_all = "camelCase")]
    Raster {
        id: f64, name: String, visible: bool, opacity: f32, blend_mode: u32,
        has_mask: bool, mask_enabled: bool, show_mask: bool,
    },
    #[serde(rename_all = "camelCase")]
    Group {
        id: f64, name: String, visible: bool, collapsed: bool, passthrough: bool,
        opacity: f32, blend_mode: u32,
        has_mask: bool, mask_enabled: bool, show_mask: bool,
        children: Vec<LayerInfo>,
    },
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VeilTypeInfo {
    #[serde(rename = "type")]
    pub type_id: &'static str,
    pub params: Vec<ParamInfo>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VeilInfo {
    #[serde(rename = "type")]
    pub type_id: String,
    pub visible: bool,
    pub index: usize,
    pub params: Vec<ParamInfo>,
}

/// Flat serialization-friendly view of a parameter definition + current value.
/// Avoids nesting a tagged enum (ParamDef) which serde_wasm_bindgen can't handle.
#[derive(serde::Serialize)]
pub struct ParamInfo {
    pub kind: &'static str,
    pub name: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    pub default: ParamValue,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<ParamValue>,
}

impl ParamInfo {
    pub fn from_def(def: &ParamDef, value: Option<&ParamValue>) -> Self {
        match def {
            ParamDef::Float { name, min, max, default } => ParamInfo {
                kind: "float", name,
                min: Some(*min as f64), max: Some(*max as f64),
                default: ParamValue::Float(*default),
                value: value.cloned(),
            },
            ParamDef::Int { name, min, max, default } => ParamInfo {
                kind: "int", name,
                min: Some(*min as f64), max: Some(*max as f64),
                default: ParamValue::Int(*default),
                value: value.cloned(),
            },
            ParamDef::Bool { name, default } => ParamInfo {
                kind: "bool", name,
                min: None, max: None,
                default: ParamValue::Bool(*default),
                value: value.cloned(),
            },
        }
    }
}

#[derive(serde::Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum StrokeOp {
    PaintCircle { x: f32, y: f32, radius: f32, r: u8, g: u8, b: u8, a: u8 },
    EraseCircle { x: f32, y: f32, radius: f32 },
    FloodFill { x: f32, y: f32, r: u8, g: u8, b: u8, a: u8, tolerance: u8 },
    LinearGradient {
        x0: f32, y0: f32, x1: f32, y1: f32,
        r0: u8, g0: u8, b0: u8, a0: u8,
        r1: u8, g1: u8, b1: u8, a1: u8,
    },
}

/// Data returned to the WASM bridge on copy/cut — always RGBA pixels regardless
/// of the internal clipboard variant.
#[derive(serde::Serialize)]
pub struct ClipboardExport {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub offset_x: i32,
    pub offset_y: i32,
}

// ---------------------------------------------------------------------------
// DarklyEngine — platform-agnostic editor core.
// ---------------------------------------------------------------------------

/// Tracks the bounding rect of a GPU stroke in progress.
struct GpuStrokeState {
    format: wgpu::TextureFormat,
    /// Tight bounding rect of all circles composited so far: [x, y, w, h].
    /// None until the first stroke_to call.
    stroke_rect: Option<[u32; 4]>,
}

impl GpuStrokeState {
    fn new(format: wgpu::TextureFormat) -> Self {
        GpuStrokeState { format, stroke_rect: None }
    }

    /// Expand the stroke rect to include a circle at (cx, cy) with the given radius.
    fn expand(&mut self, cx: f32, cy: f32, radius: f32, canvas_w: u32, canvas_h: u32) {
        let pad = 2.0; // softness + 1 pixel margin
        let x0 = (cx - radius - pad).max(0.0) as u32;
        let y0 = (cy - radius - pad).max(0.0) as u32;
        let x1 = ((cx + radius + pad).ceil() as u32).min(canvas_w);
        let y1 = ((cy + radius + pad).ceil() as u32).min(canvas_h);

        self.stroke_rect = Some(match self.stroke_rect {
            None => [x0, y0, x1 - x0, y1 - y0],
            Some([sx, sy, sw, sh]) => {
                let nx = sx.min(x0);
                let ny = sy.min(y0);
                let nx1 = (sx + sw).max(x1);
                let ny1 = (sy + sh).max(y1);
                [nx, ny, nx1 - nx, ny1 - ny]
            }
        });
    }
}

/// Pending async flood fill — waiting for GPU readback to complete.
struct PendingFloodFill {
    request: ReadbackRequest,
    layer_id: u64,
    mask_editing: bool,
    seed_x: i32,
    seed_y: i32,
    color: [u8; 4],
    tolerance: u8,
    canvas_w: u32,
    canvas_h: u32,
}

/// Pending async color pick — waiting for 1×1 GPU readback.
struct PendingColorPick {
    request: ReadbackRequest,
}

/// Pending async copy — waiting for GPU readback to build clipboard data.
struct PendingCopy {
    request: ReadbackRequest,
    /// True if copying from a mask (R8); false for layer (RGBA).
    is_mask: bool,
    /// Source region bounds in canvas coords.
    region: [u32; 4],
    /// Selection coverage for each pixel in the region (None = no selection).
    selection_data: Option<Vec<u8>>,
    /// Whether this is also a cut (clear after copy).
    is_cut: bool,
    layer_id: u64,
}

pub struct DarklyEngine {
    doc: Document,
    compositor: Compositor,
    gpu: GpuContext,
    undo_stack: UndoStack,
    active_stroke_layer: Option<u64>,
    /// Which layer has mask editing active (GIMP's `edit_mask` flag).
    /// When set, strokes are routed to the mask instead of the layer.
    editing_mask_layer: Option<u64>,
    view_transform: ViewTransform,
    /// Persistent marching ants overlay (regenerated when selection changes).
    selection_overlay: Vec<OverlayPrimitive>,
    /// Transient tool overlay (set/cleared by the active tool).
    tool_overlay: Vec<OverlayPrimitive>,
    /// Internal clipboard — holds typed content for copy/paste within Darkly.
    clipboard: Option<Clipboard>,
    /// Active floating content (paste-in-place or interactive transform).
    floating: Option<FloatingContent>,

    // --- GPU Paint Infrastructure (Phase 2) ---
    region_store: RegionStore,
    paint_pipelines: PaintPipelines,
    /// Active GPU stroke state (replaces CPU transaction for PaintCircle/EraseCircle).
    gpu_stroke: Option<GpuStrokeState>,

    // --- Async readback operations ---
    pending_flood_fill: Option<PendingFloodFill>,
    pending_color_pick: Option<PendingColorPick>,
    pending_copy: Option<PendingCopy>,
    /// Completed copy result — picked up by the frontend on the next poll.
    pending_copy_result: Option<ClipboardExport>,
    /// Last picked color — returned immediately while async readback is in flight.
    last_picked_color: [u8; 4],
}

impl DarklyEngine {
    pub fn new(gpu: GpuContext, doc_width: u32, doc_height: u32) -> Self {
        let compositor = Compositor::new(
            &gpu.device, &gpu.queue, gpu.surface_format(),
            doc_width, doc_height, gpu.is_software,
        );
        let doc = Document::new(doc_width, doc_height);
        let undo_stack = UndoStack::new(50);
        let region_store = RegionStore::new(&gpu.device, doc_width, doc_height);
        let paint_pipelines = PaintPipelines::new(&gpu.device, &gpu.queue);

        DarklyEngine {
            doc,
            compositor,
            gpu,
            undo_stack,
            active_stroke_layer: None,
            editing_mask_layer: None,
            view_transform: ViewTransform::identity(),
            selection_overlay: Vec::new(),
            tool_overlay: Vec::new(),
            clipboard: None,
            floating: None,
            region_store,
            paint_pipelines,
            gpu_stroke: None,
            pending_flood_fill: None,
            pending_color_pick: None,
            pending_copy: None,
            pending_copy_result: None,
            last_picked_color: [0, 0, 0, 0],
        }
    }

    // --- Layer CRUD ---

    pub fn add_raster_layer(&mut self) -> u64 {
        let id = self.doc.add_raster_layer();
        self.compositor.ensure_raster_layer(&self.gpu.device, &self.gpu.queue, id);
        self.compositor.mark_dirty();

        let parent = self.doc.parent_of(id);
        let pos = self.doc.position_in_parent(id).unwrap_or(0);
        self.undo_stack.push(Box::new(LayerAddAction::new(id, parent, pos)));

        id
    }

    pub fn add_raster_layer_in(&mut self, group_id: u64) -> u64 {
        let id = self.doc.add_raster_layer_in(Some(group_id));
        self.compositor.ensure_raster_layer(&self.gpu.device, &self.gpu.queue, id);
        self.compositor.mark_dirty();

        let parent = self.doc.parent_of(id);
        let pos = self.doc.position_in_parent(id).unwrap_or(0);
        self.undo_stack.push(Box::new(LayerAddAction::new(id, parent, pos)));

        id
    }

    pub fn add_group(&mut self) -> u64 {
        let id = self.doc.add_group();

        let parent = self.doc.parent_of(id);
        let pos = self.doc.position_in_parent(id).unwrap_or(0);
        self.undo_stack.push(Box::new(LayerAddAction::new(id, parent, pos)));

        id
    }

    pub fn remove_layer(&mut self, layer_id: u64) -> Result<(), String> {
        if self.doc.node_count() <= 1 {
            return Err("Cannot delete the last layer".into());
        }

        let parent = self.doc.parent_of(layer_id);
        let pos = self.doc.position_in_parent(layer_id).unwrap_or(0);

        if let Some(node) = self.doc.detach_for_undo(layer_id) {
            self.undo_stack.push(Box::new(LayerRemoveAction::new(node, parent, pos)));
        }

        self.compositor.mark_dirty();
        Ok(())
    }

    pub fn move_layer(&mut self, layer_id: u64, target: MoveTarget) {
        let old_parent = self.doc.parent_of(layer_id);
        let old_pos = match self.doc.position_in_parent(layer_id) {
            Some(p) => p,
            None => return,
        };

        self.doc.move_layer(layer_id, target);

        let new_parent = self.doc.parent_of(layer_id);
        let new_pos = self.doc.position_in_parent(layer_id).unwrap_or(0);

        self.compositor.mark_dirty();

        self.undo_stack.push(Box::new(LayerMoveAction::new(
            layer_id, old_parent, old_pos, new_parent, new_pos,
        )));
    }

    // --- Layer properties ---

    pub fn set_opacity(&mut self, layer_id: u64, opacity: f32) {
        let old_opacity = match self.doc.find_node(layer_id) {
            Some(LayerNode::Layer(Layer::Raster(r))) => r.opacity,
            Some(LayerNode::Group(g)) => g.opacity,
            _ => return,
        };

        match self.doc.find_node_mut(layer_id) {
            Some(LayerNode::Layer(Layer::Raster(r))) => r.opacity = opacity,
            Some(LayerNode::Group(g)) => g.opacity = opacity,
            _ => return,
        }

        if let Some(Layer::Raster(r)) = self.doc.layer(layer_id) {
            self.compositor.update_raster_uniforms(
                &self.gpu.queue, layer_id, r.opacity, r.blend_mode,
            );
        } else if let Some(LayerNode::Group(g)) = self.doc.find_node(layer_id) {
            self.compositor.update_group_uniforms(
                &self.gpu.queue, layer_id, g.opacity, g.blend_mode, g.show_mask,
            );
        }
        self.compositor.mark_dirty();

        self.undo_stack.coalesce_property(PropertyAction::new(
            layer_id,
            Property::Opacity(old_opacity),
            Property::Opacity(opacity),
        ));
    }

    pub fn set_blend_mode(&mut self, layer_id: u64, mode: u32) {
        let blend_mode = BlendMode::from_u32(mode);

        let old_mode = match self.doc.find_node(layer_id) {
            Some(LayerNode::Layer(Layer::Raster(r))) => r.blend_mode,
            Some(LayerNode::Group(g)) => g.blend_mode,
            _ => return,
        };

        match self.doc.find_node_mut(layer_id) {
            Some(LayerNode::Layer(Layer::Raster(r))) => r.blend_mode = blend_mode,
            Some(LayerNode::Group(g)) => g.blend_mode = blend_mode,
            _ => return,
        }

        if let Some(Layer::Raster(r)) = self.doc.layer(layer_id) {
            self.compositor.update_raster_uniforms(
                &self.gpu.queue, layer_id, r.opacity, r.blend_mode,
            );
        } else if let Some(LayerNode::Group(g)) = self.doc.find_node(layer_id) {
            self.compositor.update_group_uniforms(
                &self.gpu.queue, layer_id, g.opacity, g.blend_mode, g.show_mask,
            );
        }
        self.compositor.mark_dirty();

        self.undo_stack.push(Box::new(PropertyAction::new(
            layer_id,
            Property::BlendMode(old_mode),
            Property::BlendMode(blend_mode),
        )));
    }

    pub fn set_layer_visible(&mut self, layer_id: u64, visible: bool) {
        let old_visible = match self.doc.find_node(layer_id) {
            Some(n) => n.visible(),
            None => return,
        };

        match self.doc.find_node_mut(layer_id) {
            Some(LayerNode::Layer(Layer::Raster(r))) => r.visible = visible,
            Some(LayerNode::Group(g)) => g.visible = visible,
            _ => return,
        }
        self.compositor.mark_dirty();

        self.undo_stack.push(Box::new(PropertyAction::new(
            layer_id,
            Property::Visible(old_visible),
            Property::Visible(visible),
        )));
    }

    pub fn set_layer_name(&mut self, layer_id: u64, name: &str) {
        let old_name = match self.doc.find_node(layer_id) {
            Some(LayerNode::Layer(Layer::Raster(r))) => r.name.clone(),
            Some(LayerNode::Group(g)) => g.name.clone(),
            _ => return,
        };

        match self.doc.find_node_mut(layer_id) {
            Some(LayerNode::Layer(Layer::Raster(r))) => r.name = name.to_string(),
            Some(LayerNode::Group(g)) => g.name = name.to_string(),
            _ => return,
        }

        self.undo_stack.push(Box::new(PropertyAction::new(
            layer_id,
            Property::Name(old_name),
            Property::Name(name.to_string()),
        )));
    }

    pub fn set_group_collapsed(&mut self, group_id: u64, collapsed: bool) {
        if let Some(LayerNode::Group(g)) = self.doc.find_node_mut(group_id) {
            g.collapsed = collapsed;
        }
    }

    pub fn set_group_passthrough(&mut self, group_id: u64, passthrough: bool) {
        let old = match self.doc.find_node(group_id) {
            Some(LayerNode::Group(g)) => g.passthrough,
            _ => return,
        };
        if let Some(LayerNode::Group(g)) = self.doc.find_node_mut(group_id) {
            g.passthrough = passthrough;
        }
        if !passthrough {
            self.compositor.ensure_group_state(&self.gpu.device, &self.gpu.queue, group_id);
            if let Some(LayerNode::Group(g)) = self.doc.find_node(group_id) {
                self.compositor.update_group_uniforms(
                    &self.gpu.queue, group_id, g.opacity, g.blend_mode, g.show_mask,
                );
            }
        }
        self.compositor.mark_dirty();
        self.undo_stack.push(Box::new(PropertyAction::new(
            group_id,
            Property::Passthrough(old),
            Property::Passthrough(passthrough),
        )));
    }

    // --- Layer Masks ---

    pub fn add_mask(&mut self, layer_id: u64) {

        // Snapshot old state for undo
        let node = match self.doc.find_node(layer_id) {
            Some(n) => n,
            None => return,
        };
        let m = node.as_masked();
        let (old_mask, old_enabled, old_show) = (m.mask().clone(), m.mask_enabled(), m.show_mask());

        self.doc.add_mask(layer_id);
        self.compositor.set_layer_mask(&self.gpu.device, &self.gpu.queue, layer_id, true);
        self.sync_mask_state(layer_id);
        self.compositor.mark_dirty();

        self.undo_stack.push(Box::new(MaskPropertyAction::new(
            layer_id, old_mask, old_enabled, old_show,
        )));
    }

    pub fn remove_mask(&mut self, layer_id: u64) {

        let node = match self.doc.find_node(layer_id) {
            Some(n) => n,
            None => return,
        };
        let m = node.as_masked();
        let (old_mask, old_enabled, old_show) = (m.mask().clone(), m.mask_enabled(), m.show_mask());

        self.doc.remove_mask(layer_id);
        self.editing_mask_layer = self.editing_mask_layer.filter(|&id| id != layer_id);
        self.compositor.set_layer_mask(&self.gpu.device, &self.gpu.queue, layer_id, false);
        self.sync_mask_state(layer_id);
        self.compositor.mark_dirty();

        self.undo_stack.push(Box::new(MaskPropertyAction::new(
            layer_id, old_mask, old_enabled, old_show,
        )));
    }

    pub fn apply_mask(&mut self, layer_id: u64) {
        // apply_mask is raster-only — groups have no pixel data to bake into
        let (old_mask, old_enabled, old_show) = match self.doc.layer(layer_id) {
            Some(Layer::Raster(r)) => (r.mask.clone(), r.mask_enabled, r.show_mask),
            _ => return,
        };
        if old_mask.is_none() {
            return;
        }

        let canvas_w = self.doc.width;
        let canvas_h = self.doc.height;
        let format = wgpu::TextureFormat::Rgba8Unorm;

        // Save layer texture to region store for undo.
        if let Some(layer_tex) = self.compositor.layer_texture(layer_id) {
            let mut encoder = self.gpu.device.create_command_encoder(
                &wgpu::CommandEncoderDescriptor { label: Some("apply-mask-save") },
            );
            self.region_store.save_region(
                &mut encoder, &layer_tex.texture, format,
                [0, 0, canvas_w, canvas_h],
            );
            self.gpu.queue.submit([encoder.finish()]);
        }

        // Create a bind group from the mask GPU texture for the multiply pass.
        let mask_bind_group = self.compositor.mask_texture(layer_id).map(|mask_tex| {
            let sampler = self.gpu.device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("mask-apply-sampler"),
                mag_filter: wgpu::FilterMode::Nearest,
                min_filter: wgpu::FilterMode::Nearest,
                ..Default::default()
            });
            self.paint_pipelines.create_selection_bind_group(
                &self.gpu.device, &mask_tex.view, &sampler,
            )
        });

        // GPU render pass: multiply layer alpha by mask values.
        if let (Some(layer_tex), Some(mask_bg)) = (
            self.compositor.layer_texture(layer_id),
            mask_bind_group.as_ref(),
        ) {
            let target = GpuPaintTarget::from_layer(layer_tex, canvas_w, canvas_h);
            let mut encoder = self.gpu.device.create_command_encoder(
                &wgpu::CommandEncoderDescriptor { label: Some("apply-mask-multiply") },
            );
            target.multiply_by_mask(
                &mut encoder, &self.paint_pipelines, &self.gpu.queue, mask_bg,
            );
            self.gpu.queue.submit([encoder.finish()]);
        }

        // Commit undo region.
        {
            let mut encoder = self.gpu.device.create_command_encoder(
                &wgpu::CommandEncoderDescriptor { label: Some("apply-mask-undo") },
            );
            let entry = self.region_store.commit_region(
                &mut encoder, layer_id, format, [0, 0, canvas_w, canvas_h],
            );
            self.gpu.queue.submit([encoder.finish()]);
            self.undo_stack.push(Box::new(GpuRegionAction::new(entry)));
        }

        self.editing_mask_layer = self.editing_mask_layer.filter(|&id| id != layer_id);
        self.compositor.set_layer_mask(&self.gpu.device, &self.gpu.queue, layer_id, false);
        self.sync_mask_state(layer_id);
        self.compositor.mark_dirty();

        self.undo_stack.push(Box::new(MaskPropertyAction::new(
            layer_id, old_mask, old_enabled, old_show,
        )));
    }

    pub fn set_mask_enabled(&mut self, layer_id: u64, enabled: bool) {

        let old = match self.doc.find_node(layer_id) {
            Some(n) => n.as_masked().mask_enabled(),
            None => return,
        };
        self.doc.set_mask_enabled(layer_id, enabled);
        self.sync_mask_state(layer_id);
        self.compositor.mark_dirty();

        self.undo_stack.push(Box::new(MaskPropertyAction::new(
            layer_id, None, old, false,
        )));
    }

    pub fn set_show_mask(&mut self, layer_id: u64, show: bool) {

        let old = match self.doc.find_node(layer_id) {
            Some(n) => n.as_masked().show_mask(),
            None => return,
        };
        self.doc.set_show_mask(layer_id, show);
        self.sync_mask_state(layer_id);
        self.compositor.mark_dirty();

        self.undo_stack.push(Box::new(MaskPropertyAction::new(
            layer_id, None, false, old,
        )));
    }

    pub fn set_editing_mask(&mut self, layer_id: u64, editing: bool) {
        if editing {
            self.editing_mask_layer = Some(layer_id);
        } else if self.editing_mask_layer == Some(layer_id) {
            self.editing_mask_layer = None;
        }
    }

    pub fn selection_to_mask(&mut self, layer_id: u64) {

        let node = match self.doc.find_node(layer_id) {
            Some(n) => n,
            None => return,
        };
        let m = node.as_masked();
        let (old_mask, old_enabled, old_show) = (m.mask().clone(), m.mask_enabled(), m.show_mask());

        self.doc.selection_to_mask(layer_id);
        self.compositor.set_layer_mask(&self.gpu.device, &self.gpu.queue, layer_id, true);

        // Upload selection data directly to the GPU mask texture.
        self.upload_mask_to_gpu(layer_id);

        self.sync_mask_state(layer_id);
        self.compositor.mark_dirty();

        self.undo_stack.push(Box::new(MaskPropertyAction::new(
            layer_id, old_mask, old_enabled, old_show,
        )));
    }

    pub fn mask_to_selection(&mut self, layer_id: u64) {
        let old_sel = self.doc.selection.clone();

        // Readback the GPU mask texture and build an AlphaMask from the bytes.
        if let Some(mask_tex) = self.compositor.mask_texture(layer_id) {
            let canvas_w = self.doc.width;
            let canvas_h = self.doc.height;
            let mut encoder = self.gpu.device.create_command_encoder(
                &wgpu::CommandEncoderDescriptor { label: Some("mask-to-sel-readback") },
            );
            let request = readback::request_readback(
                &self.gpu.device, &mut encoder, &mask_tex.texture,
                wgpu::TextureFormat::R8Unorm,
                [0, 0, canvas_w, canvas_h],
            );
            self.gpu.queue.submit([encoder.finish()]);
            let pixels = request.blocking_read(&self.gpu.device);

            // Build AlphaMask from R8 pixel data.
            let ts = TILE_SIZE;
            let mut mask = AlphaMask::new();
            for py in 0..canvas_h {
                for px in 0..canvas_w {
                    let v = pixels[(py * canvas_w + px) as usize];
                    // Only store non-zero values (skip fully-transparent regions).
                    if v > 0 {
                        let tx = (px / ts as u32) as i32;
                        let ty = (py / ts as u32) as i32;
                        let lx = (px % ts as u32) as usize;
                        let ly = (py % ts as u32) as usize;
                        mask.get_or_create(tx, ty).write().set(lx, ly, v as f32 / 255.0);
                    }
                }
            }
            self.doc.selection = Some(mask);
        } else {
            // Fallback: use CPU mask data.
            self.doc.mask_to_selection(layer_id);
        }

        self.undo_stack.push(Box::new(SelectionAction::new(old_sel)));
        self.update_selection_overlay();
    }

    /// Upload CPU-side layer tiles (RGBA8) to the GPU layer texture.
    fn upload_layer_tiles_to_gpu(&self, layer_id: u64) {
        let layer_tex = match self.compositor.layer_texture(layer_id) {
            Some(t) => t,
            None => return,
        };
        let raster = match self.doc.layer(layer_id) {
            Some(Layer::Raster(r)) => r,
            _ => return,
        };
        let ts = TILE_SIZE;
        for ((tx, ty), tile) in raster.surface.store.iter() {
            if tx < 0 || ty < 0 { continue; }
            if tx as u32 >= layer_tex.width_in_tiles || ty as u32 >= layer_tex.height_in_tiles {
                continue;
            }
            self.gpu.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &layer_tex.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: tx as u32 * ts as u32,
                        y: ty as u32 * ts as u32,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                &tile.data().0,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(ts as u32 * 4),
                    rows_per_image: None,
                },
                wgpu::Extent3d {
                    width: ts as u32,
                    height: ts as u32,
                    depth_or_array_layers: 1,
                },
            );
        }
    }

    /// Upload CPU-side mask data (AlphaMask) to the GPU mask texture.
    fn upload_mask_to_gpu(&self, layer_id: u64) {
        let mask_tex = match self.compositor.mask_texture(layer_id) {
            Some(t) => t,
            None => return,
        };
        let node = match self.doc.find_node(layer_id) {
            Some(n) => n,
            None => return,
        };
        let mask_store = match node.as_masked().mask() {
            Some(m) => &m.store,
            None => return,
        };

        let ts = TILE_SIZE;
        // Thread-local buffer for f32→u8 conversion.
        let mut buf = vec![0u8; ts * ts];

        for ((tx, ty), tile) in mask_store.iter() {
            if tx < 0 || ty < 0 { continue; }
            if tx as u32 >= mask_tex.width_in_tiles || ty as u32 >= mask_tex.height_in_tiles {
                continue;
            }
            let data = tile.data();
            for i in 0..(ts * ts) {
                buf[i] = (data.0[i].clamp(0.0, 1.0) * 255.0) as u8;
            }
            self.gpu.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &mask_tex.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: tx as u32 * ts as u32,
                        y: ty as u32 * ts as u32,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                &buf,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(ts as u32),
                    rows_per_image: None,
                },
                wgpu::Extent3d {
                    width: ts as u32,
                    height: ts as u32,
                    depth_or_array_layers: 1,
                },
            );
        }
    }

    /// Sync compositor mask state (bind group + uniforms) for a layer or group.
    fn sync_mask_state(&mut self, layer_id: u64) {

        let node = match self.doc.find_node(layer_id) {
            Some(n) => n,
            None => return,
        };
        let m = node.as_masked();
        let has_mask = m.mask().is_some();
        let mask_enabled = m.mask_enabled();
        let show_mask = m.show_mask();

        self.compositor.set_layer_mask(&self.gpu.device, &self.gpu.queue, layer_id, has_mask);
        self.compositor.update_mask_binding(
            &self.gpu.device, layer_id, mask_enabled, show_mask,
        );

        // Update uniforms for the appropriate cache type
        match node {
            LayerNode::Layer(Layer::Raster(r)) => {
                self.compositor.update_raster_uniforms_full(
                    &self.gpu.queue, layer_id, r.opacity, r.blend_mode, show_mask,
                );
            }
            LayerNode::Group(g) => {
                self.compositor.update_group_uniforms(
                    &self.gpu.queue, layer_id, g.opacity, g.blend_mode, show_mask,
                );
            }
        }
    }

    // --- Painting ---

    pub fn paint(
        &mut self,
        layer_id: u64,
        x: f32, y: f32, radius: f32,
        r: u8, g: u8, b: u8, a: u8,
    ) {
        self.doc.paint_circle(layer_id, x, y, radius, [r, g, b, a]);
    }

    pub fn fill_gradient(&mut self, layer_id: u64) {
        self.doc.fill_gradient(layer_id);
        self.upload_layer_tiles_to_gpu(layer_id);
        self.compositor.mark_dirty();
    }

    // --- Stroke lifecycle ---
    // Following GIMP's edit_mask flag: when editing_mask_layer is set,
    // strokes are routed to the mask instead of the layer.
    //
    // All stroke ops go through GPU render passes (Phase 3).

    pub fn begin_stroke(&mut self, layer_id: u64) {
        self.auto_commit_floating();
        self.doc.set_mask_editing(
            if self.editing_mask_layer == Some(layer_id) { Some(layer_id) } else { None }
        );
        self.active_stroke_layer = Some(layer_id);
        // GPU setup is deferred to first stroke_to (lazy init).
    }

    pub fn stroke_to(&mut self, op: StrokeOp) {
        let layer_id = match self.active_stroke_layer {
            Some(id) => id,
            None => return,
        };
        self.gpu_stroke_to(layer_id, op);
    }

    /// GPU paint path for all stroke operations.
    fn gpu_stroke_to(&mut self, layer_id: u64, op: StrokeOp) {
        let mask_editing = self.editing_mask_layer == Some(layer_id);
        let canvas_w = self.compositor.canvas_width();
        let canvas_h = self.compositor.canvas_height();

        // Lazy init: save the region on first stroke_to.
        if self.gpu_stroke.is_none() {
            let (texture, format) = if mask_editing {
                match self.compositor.mask_texture(layer_id) {
                    Some(t) => (&t.texture, wgpu::TextureFormat::R8Unorm),
                    None => return,
                }
            } else {
                match self.compositor.layer_texture(layer_id) {
                    Some(t) => (&t.texture, wgpu::TextureFormat::Rgba8Unorm),
                    None => return,
                }
            };

            // Save the entire canvas to scratch for undo.
            let mut encoder = self.gpu.device.create_command_encoder(
                &wgpu::CommandEncoderDescriptor { label: Some("stroke-begin") },
            );
            self.region_store.save_region(&mut encoder, texture, format, [0, 0, canvas_w, canvas_h]);
            self.gpu.queue.submit([encoder.finish()]);

            self.gpu_stroke = Some(GpuStrokeState::new(format));
        }

        // Helper closure to create a paint target from compositor textures.
        // Defined as a macro to avoid holding borrows across match arms.
        macro_rules! paint_target {
            () => {
                if mask_editing {
                    self.compositor.mask_texture(layer_id)
                        .map(|t| GpuPaintTarget::from_mask(t, canvas_w, canvas_h))
                } else {
                    self.compositor.layer_texture(layer_id)
                        .map(|t| GpuPaintTarget::from_layer(t, canvas_w, canvas_h))
                }
            };
        }

        match op {
            StrokeOp::PaintCircle { x, y, radius, r, g, b, a } => {
                let target = match paint_target!() { Some(t) => t, None => return };
                let mut encoder = self.gpu.device.create_command_encoder(
                    &wgpu::CommandEncoderDescriptor { label: Some("stroke-to") },
                );
                target.composite_circle(
                    &mut encoder, &self.paint_pipelines, &self.gpu.queue,
                    x, y, radius, [r, g, b, a], 1.0,
                );
                self.gpu.queue.submit([encoder.finish()]);
                if let Some(gs) = &mut self.gpu_stroke {
                    gs.expand(x, y, radius, canvas_w, canvas_h);
                }
            }
            StrokeOp::EraseCircle { x, y, radius } => {
                let target = match paint_target!() { Some(t) => t, None => return };
                let mut encoder = self.gpu.device.create_command_encoder(
                    &wgpu::CommandEncoderDescriptor { label: Some("stroke-to") },
                );
                target.erase_circle(
                    &mut encoder, &self.paint_pipelines, &self.gpu.queue,
                    x, y, radius,
                );
                self.gpu.queue.submit([encoder.finish()]);
                if let Some(gs) = &mut self.gpu_stroke {
                    gs.expand(x, y, radius, canvas_w, canvas_h);
                }
            }
            StrokeOp::LinearGradient { x0, y0, x1, y1, r0, g0, b0, a0, r1, g1, b1, a1 } => {
                let target = match paint_target!() { Some(t) => t, None => return };
                let mut encoder = self.gpu.device.create_command_encoder(
                    &wgpu::CommandEncoderDescriptor { label: Some("stroke-gradient") },
                );
                target.linear_gradient(
                    &mut encoder, &self.paint_pipelines, &self.gpu.queue,
                    x0, y0, x1, y1, [r0, g0, b0, a0], [r1, g1, b1, a1], None,
                );
                self.gpu.queue.submit([encoder.finish()]);
                // Gradient covers the full canvas.
                if let Some(gs) = &mut self.gpu_stroke {
                    gs.stroke_rect = Some([0, 0, canvas_w, canvas_h]);
                }
            }
            StrokeOp::FloodFill { x, y, r, g, b, a, tolerance } => {
                // Flood fill needs mutable self access, so the target is obtained inside.
                self.gpu_flood_fill(layer_id, mask_editing,
                    x as i32, y as i32, [r, g, b, a], tolerance,
                    canvas_w, canvas_h);
            }
        }

        self.compositor.mark_dirty();
    }

    /// Start async GPU flood fill: readback layer texture, then complete on a
    /// subsequent frame when the data arrives.
    fn gpu_flood_fill(
        &mut self,
        layer_id: u64,
        mask_editing: bool,
        seed_x: i32,
        seed_y: i32,
        color: [u8; 4],
        tolerance: u8,
        canvas_w: u32,
        canvas_h: u32,
    ) {
        let (target, format) = match self.get_paint_target(layer_id, mask_editing) {
            Some(t) => t,
            None => return,
        };
        let mut encoder = self.gpu.device.create_command_encoder(
            &wgpu::CommandEncoderDescriptor { label: Some("flood-fill-readback") },
        );
        let mut request = readback::request_readback(
            &self.gpu.device, &mut encoder, target.texture, format,
            [0, 0, canvas_w, canvas_h],
        );
        self.gpu.queue.submit([encoder.finish()]);
        request.begin_mapping();

        self.pending_flood_fill = Some(PendingFloodFill {
            request,
            layer_id,
            mask_editing,
            seed_x,
            seed_y,
            color,
            tolerance,
            canvas_w,
            canvas_h,
        });
    }

    /// Complete a pending flood fill once readback data is available.
    fn complete_flood_fill(&mut self, pending: PendingFloodFill, pixels: Vec<u8>) {
        let PendingFloodFill {
            layer_id, mask_editing, seed_x, seed_y, color, tolerance,
            canvas_w, canvas_h, ..
        } = pending;

        // 1. CPU scanline fill → produce R8 mask.
        let fill_mask = if mask_editing {
            flood_fill::flood_fill_r8(&pixels, canvas_w, canvas_h, seed_x, seed_y, tolerance)
        } else {
            flood_fill::flood_fill_rgba(&pixels, canvas_w, canvas_h, seed_x, seed_y, tolerance)
        };

        // 2. Upload fill mask and stamp onto target.
        let mask_bind_group = self.paint_pipelines.upload_r8_bind_group(
            &self.gpu.device, &self.gpu.queue, canvas_w, canvas_h,
            &fill_mask, "flood-fill-mask",
        );

        let (target, _) = match self.get_paint_target(layer_id, mask_editing) {
            Some(t) => t,
            None => return,
        };

        let mut encoder = self.gpu.device.create_command_encoder(
            &wgpu::CommandEncoderDescriptor { label: Some("flood-fill-stamp") },
        );
        target.fill_rect_with_selection(
            &mut encoder, &self.paint_pipelines, &self.gpu.queue,
            [0, 0, canvas_w, canvas_h], color, &mask_bind_group,
        );
        self.gpu.queue.submit([encoder.finish()]);

        // 4. Commit undo — the stroke lifecycle was deferred for async fill.
        if let Some(gs) = self.gpu_stroke.take() {
            let rect = [0u32, 0, canvas_w, canvas_h];
            let mut encoder = self.gpu.device.create_command_encoder(
                &wgpu::CommandEncoderDescriptor { label: Some("flood-fill-undo") },
            );
            let entry = self.region_store.commit_region(
                &mut encoder, layer_id, gs.format, rect,
            );
            self.gpu.queue.submit([encoder.finish()]);
            self.undo_stack.push(Box::new(GpuRegionAction::new(entry)));
        }

        self.compositor.mark_dirty();
    }

    /// Complete a pending copy once GPU readback data is available.
    fn complete_copy(&mut self, pending: PendingCopy, pixels: Vec<u8>) {
        let PendingCopy {
            is_mask, region, selection_data, is_cut, layer_id, ..
        } = pending;
        let [rx, ry, rw, rh] = region;

        // Build RGBA bytes from the readback data.
        let (rgba, width, height) = if is_mask {
            // R8 readback → convert to grayscale RGBA: [v, v, v, 255]
            let mut rgba = vec![0u8; (rw * rh * 4) as usize];
            for i in 0..(rw * rh) as usize {
                let v = pixels[i];
                // Skip fully-revealed mask pixels (default state).
                if v == 255 && selection_data.is_none() {
                    // For masks, 255 = "reveal all" which is the default.
                    // Only include if selection forces inclusion.
                } else {
                    let sv = if let Some(ref sel) = selection_data {
                        let coverage = sel[i] as f32 / 255.0;
                        ((v as f32 * coverage).round()) as u8
                    } else {
                        v
                    };
                    if sv > 0 {
                        rgba[i * 4] = sv;
                        rgba[i * 4 + 1] = sv;
                        rgba[i * 4 + 2] = sv;
                        rgba[i * 4 + 3] = 255;
                    }
                }
            }
            (rgba, rw, rh)
        } else {
            // RGBA readback. Apply selection masking if present.
            let mut rgba = pixels;
            if let Some(ref sel) = selection_data {
                for i in 0..(rw * rh) as usize {
                    let coverage = sel[i] as f32 / 255.0;
                    if coverage <= 0.0 {
                        rgba[i * 4] = 0;
                        rgba[i * 4 + 1] = 0;
                        rgba[i * 4 + 2] = 0;
                        rgba[i * 4 + 3] = 0;
                    } else if coverage < 1.0 {
                        // Multiply alpha by selection coverage.
                        let a = rgba[i * 4 + 3] as f32 * coverage;
                        rgba[i * 4 + 3] = a.round() as u8;
                    }
                }
            }
            (rgba, rw, rh)
        };

        let offset_x = rx as i32;
        let offset_y = ry as i32;

        // Build ImageClip and store in clipboard.
        let clip = ImageClip::from_rgba(width, height, &rgba, offset_x, offset_y);
        let (export_rgba, ew, eh, eox, eoy) = clip.to_rgba();
        self.clipboard = Some(Clipboard::ImageData(clip));

        self.pending_copy_result = Some(ClipboardExport {
            rgba: export_rgba,
            width: ew,
            height: eh,
            offset_x: eox,
            offset_y: eoy,
        });

        // If this was a cut, clear the source.
        if is_cut {
            if self.doc.selection.is_some() {
                self.gpu_clear_selection(layer_id);
            } else {
                self.gpu_clear_layer(layer_id);
            }
        }
    }

    pub fn end_stroke(&mut self) {
        if let Some(layer_id) = self.active_stroke_layer.take() {
            // If a flood fill is pending, defer undo commit — complete_flood_fill
            // will handle it when the readback arrives.
            if self.pending_flood_fill.is_some() {
                self.doc.set_mask_editing(None);
                return;
            }

            if let Some(gs) = self.gpu_stroke.take() {
                // GPU path: commit the changed region to the undo buffer.
                if let Some(rect) = gs.stroke_rect {
                    let mut encoder = self.gpu.device.create_command_encoder(
                        &wgpu::CommandEncoderDescriptor { label: Some("stroke-end") },
                    );
                    let entry = self.region_store.commit_region(
                        &mut encoder, layer_id, gs.format, rect,
                    );
                    self.gpu.queue.submit([encoder.finish()]);
                    self.undo_stack.push(Box::new(GpuRegionAction::new(entry)));
                }
                // else: no paint was applied (empty stroke), nothing to undo.
            }
            self.doc.set_mask_editing(None);
        }
    }

    // --- GPU erase helpers ---

    /// Clear layer pixels within the current selection via GPU erase pass.
    fn gpu_clear_selection(&mut self, layer_id: u64) {
        let canvas_w = self.compositor.canvas_width();
        let canvas_h = self.compositor.canvas_height();
        let mask_editing = self.editing_mask_layer == Some(layer_id);

        let (target, format) = match self.get_paint_target(layer_id, mask_editing) {
            Some(t) => t,
            None => return,
        };

        // Upload selection mask as R8 GPU texture.
        let sel_bind_group = match self.upload_selection_mask(canvas_w, canvas_h) {
            Some(bg) => bg,
            None => return,
        };

        // Save region for undo.
        let mut encoder = self.gpu.device.create_command_encoder(
            &wgpu::CommandEncoderDescriptor { label: Some("clear-sel-save") },
        );
        self.region_store.save_region(&mut encoder, target.texture, format, [0, 0, canvas_w, canvas_h]);
        self.gpu.queue.submit([encoder.finish()]);

        // Erase within selection.
        let mut encoder = self.gpu.device.create_command_encoder(
            &wgpu::CommandEncoderDescriptor { label: Some("clear-sel-erase") },
        );
        target.erase_with_selection(
            &mut encoder, &self.paint_pipelines, &self.gpu.queue, &sel_bind_group,
        );
        self.gpu.queue.submit([encoder.finish()]);

        // Commit for undo.
        let mut encoder = self.gpu.device.create_command_encoder(
            &wgpu::CommandEncoderDescriptor { label: Some("clear-sel-commit") },
        );
        let entry = self.region_store.commit_region(
            &mut encoder, layer_id, format, [0, 0, canvas_w, canvas_h],
        );
        self.gpu.queue.submit([encoder.finish()]);
        self.undo_stack.push(Box::new(GpuRegionAction::new(entry)));
        self.compositor.mark_dirty();
    }

    /// Clear entire layer to transparent via GPU.
    fn gpu_clear_layer(&mut self, layer_id: u64) {
        let canvas_w = self.compositor.canvas_width();
        let canvas_h = self.compositor.canvas_height();
        let mask_editing = self.editing_mask_layer == Some(layer_id);

        let (target, format) = match self.get_paint_target(layer_id, mask_editing) {
            Some(t) => t,
            None => return,
        };

        // Save region for undo.
        let mut encoder = self.gpu.device.create_command_encoder(
            &wgpu::CommandEncoderDescriptor { label: Some("clear-layer-save") },
        );
        self.region_store.save_region(&mut encoder, target.texture, format, [0, 0, canvas_w, canvas_h]);
        self.gpu.queue.submit([encoder.finish()]);

        // Clear the full canvas.
        let mut encoder = self.gpu.device.create_command_encoder(
            &wgpu::CommandEncoderDescriptor { label: Some("clear-layer") },
        );
        target.clear_rect(
            &mut encoder, &self.paint_pipelines, &self.gpu.queue,
            [0, 0, canvas_w, canvas_h],
        );
        self.gpu.queue.submit([encoder.finish()]);

        // Commit for undo.
        let mut encoder = self.gpu.device.create_command_encoder(
            &wgpu::CommandEncoderDescriptor { label: Some("clear-layer-commit") },
        );
        let entry = self.region_store.commit_region(
            &mut encoder, layer_id, format, [0, 0, canvas_w, canvas_h],
        );
        self.gpu.queue.submit([encoder.finish()]);
        self.undo_stack.push(Box::new(GpuRegionAction::new(entry)));
        self.compositor.mark_dirty();
    }

    /// Get a GpuPaintTarget for a layer (or its mask), plus its format.
    fn get_paint_target(&self, layer_id: u64, mask_editing: bool) -> Option<(GpuPaintTarget<'_>, wgpu::TextureFormat)> {
        let canvas_w = self.compositor.canvas_width();
        let canvas_h = self.compositor.canvas_height();
        if mask_editing {
            self.compositor.mask_texture(layer_id)
                .map(|t| (GpuPaintTarget::from_mask(t, canvas_w, canvas_h), wgpu::TextureFormat::R8Unorm))
        } else {
            self.compositor.layer_texture(layer_id)
                .map(|t| (GpuPaintTarget::from_layer(t, canvas_w, canvas_h), wgpu::TextureFormat::Rgba8Unorm))
        }
    }

    /// Upload a cropped region of the selection mask as an R8 GPU texture.
    /// The output matches the given sub-region dimensions for use with
    /// `multiply_by_mask` on a source texture that covers only that region.
    fn upload_cropped_selection_mask(
        &self,
        origin: (i32, i32),
        width: u32,
        height: u32,
    ) -> Option<wgpu::BindGroup> {
        let selection = self.doc.selection.as_ref()?;

        let ts = TILE_SIZE;
        let mut pixels = vec![0u8; (width * height) as usize];
        let (ox, oy) = origin;

        for ((tx, ty), tile) in selection.iter() {
            let base_x = tx * ts as i32;
            let base_y = ty * ts as i32;
            let data = tile.data();
            for ly in 0..ts {
                for lx in 0..ts {
                    let cx = base_x + lx as i32;
                    let cy = base_y + ly as i32;
                    let px = cx - ox;
                    let py = cy - oy;
                    if px >= 0 && py >= 0 && (px as u32) < width && (py as u32) < height {
                        let v = (data.get(lx, ly) * 255.0).clamp(0.0, 255.0) as u8;
                        pixels[(py as u32 * width + px as u32) as usize] = v;
                    }
                }
            }
        }

        Some(self.paint_pipelines.upload_r8_bind_group(
            &self.gpu.device, &self.gpu.queue, width, height,
            &pixels, "selection-cropped",
        ))
    }

    /// Upload the document's selection mask (AlphaMask) as an R8 GPU texture,
    /// returning a bind group suitable for the paint pipeline's selection slot.
    fn upload_selection_mask(&self, canvas_w: u32, canvas_h: u32) -> Option<wgpu::BindGroup> {
        let selection = self.doc.selection.as_ref()?;

        // Rasterize AlphaMask tiles to flat R8 buffer.
        let ts = TILE_SIZE;
        let mut pixels = vec![0u8; (canvas_w * canvas_h) as usize];
        for ((tx, ty), tile) in selection.iter() {
            let base_x = (tx * ts as i32) as u32;
            let base_y = (ty * ts as i32) as u32;
            let data = tile.data();
            for ly in 0..ts {
                for lx in 0..ts {
                    let px = base_x + lx as u32;
                    let py = base_y + ly as u32;
                    if px < canvas_w && py < canvas_h {
                        let v = (data.get(lx, ly) * 255.0).clamp(0.0, 255.0) as u8;
                        pixels[(py * canvas_w + px) as usize] = v;
                    }
                }
            }
        }

        Some(self.paint_pipelines.upload_r8_bind_group(
            &self.gpu.device, &self.gpu.queue, canvas_w, canvas_h,
            &pixels, "selection-upload",
        ))
    }

    // --- View transform ---

    pub fn set_view_transform(
        &mut self,
        pan_x: f32, pan_y: f32,
        zoom: f32, rotation: f32,
        screen_w: f32, screen_h: f32,
    ) {
        let transform = ViewTransform::from_pan_zoom_rotate(
            pan_x, pan_y, zoom, rotation,
            screen_w, screen_h,
            self.doc.width as f32, self.doc.height as f32,
        );
        self.view_transform = transform;
        self.compositor.update_view_transform(&self.gpu.queue, &transform);
        self.compositor.mark_needs_present();
    }

    pub fn screen_to_canvas(&self, screen_x: f32, screen_y: f32) -> (f32, f32) {
        self.view_transform.screen_to_canvas(screen_x, screen_y)
    }

    /// Start an async color pick at canvas coordinates.
    ///
    /// Returns the last picked color immediately (for responsive UI) and kicks
    /// off a readback.  The result will be available via [`poll_pending`] on
    /// the next frame and cached in `last_picked_color`.
    pub fn pick_color(&mut self, x: f32, y: f32) -> [u8; 4] {
        let canvas_w = self.compositor.canvas_width();
        let canvas_h = self.compositor.canvas_height();
        let px = x as u32;
        let py = y as u32;

        if px >= canvas_w || py >= canvas_h {
            return [0, 0, 0, 0];
        }

        let texture = self.compositor.composited_texture();
        let mut encoder = self.gpu.device.create_command_encoder(
            &wgpu::CommandEncoderDescriptor { label: Some("pick-color") },
        );
        let mut request = readback::request_readback(
            &self.gpu.device, &mut encoder, texture,
            wgpu::TextureFormat::Rgba8Unorm, [px, py, 1, 1],
        );
        self.gpu.queue.submit([encoder.finish()]);
        request.begin_mapping();

        self.pending_color_pick = Some(PendingColorPick { request });

        // Return cached color for immediate feedback — real result arrives next frame.
        self.last_picked_color
    }

    // --- Thumbnails ---

    /// Generate an RGBA thumbnail of a layer's content.
    /// Returns `width * height * 4` bytes (RGBA). Transparent areas show checkerboard.
    pub fn layer_thumbnail(&self, layer_id: u64, width: u32, height: u32) -> Vec<u8> {
        let tiles = match self.doc.layer(layer_id) {
            Some(Layer::Raster(r)) => &r.surface.store,
            _ => return vec![0u8; (width * height * 4) as usize],
        };
        generate_rgba_thumbnail(tiles, self.doc.width, self.doc.height, width, height)
    }

    /// Generate an RGBA thumbnail of a layer's mask (grayscale).
    /// Returns empty vec if the layer has no mask.
    pub fn mask_thumbnail(&self, layer_id: u64, width: u32, height: u32) -> Vec<u8> {
        let mask_store = match self.doc.find_node(layer_id)
            .and_then(|n| n.as_masked().mask().as_ref())
        {
            Some(m) => &m.store,
            None => return Vec::new(),
        };
        generate_mask_thumbnail(mask_store, self.doc.width, self.doc.height, width, height)
    }

    // --- Rendering ---

    /// Poll pending async readback operations (flood fill, color pick).
    ///
    /// Called at the start of each frame.  Returns true if any operation
    /// completed (and therefore the compositor should re-render).
    fn poll_pending(&mut self) -> bool {
        let mut did_work = false;

        // --- Flood fill ---
        if let Some(ref pending) = self.pending_flood_fill {
            if let Some(pixels) = pending.request.poll(&self.gpu.device) {
                let pending = self.pending_flood_fill.take().unwrap();
                self.complete_flood_fill(pending, pixels);
                did_work = true;
            }
        }

        // --- Color pick ---
        if let Some(ref pending) = self.pending_color_pick {
            if let Some(pixels) = pending.request.poll(&self.gpu.device) {
                if pixels.len() >= 4 {
                    self.last_picked_color = [pixels[0], pixels[1], pixels[2], pixels[3]];
                }
                self.pending_color_pick = None;
                did_work = true;
            }
        }

        // --- Copy readback ---
        if let Some(ref pending) = self.pending_copy {
            if let Some(pixels) = pending.request.poll(&self.gpu.device) {
                let pending = self.pending_copy.take().unwrap();
                self.complete_copy(pending, pixels);
                did_work = true;
            }
        }

        did_work
    }

    /// Get the most recently picked color (updated asynchronously).
    pub fn last_picked_color(&self) -> [u8; 4] {
        self.last_picked_color
    }

    /// True if a color pick readback is still in flight.
    pub fn has_pending_color_pick(&self) -> bool {
        self.pending_color_pick.is_some()
    }

    /// Render a frame. Returns true if animations need another frame.
    pub fn render(&mut self, time_secs: f32) -> bool {
        let pending_completed = self.poll_pending();
        if pending_completed {
            self.compositor.mark_dirty();
        }

        self.compositor.update_animations(&self.gpu.queue, time_secs);
        self.compositor.render(
            &self.gpu.device,
            &self.gpu.queue,
            &self.gpu.surface,
            &self.gpu.surface_config,
            &mut self.doc,
        );

        // Keep requesting frames while async operations are in flight.
        self.compositor.needs_animation()
            || self.pending_flood_fill.is_some()
            || self.pending_color_pick.is_some()
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.gpu.resize(width, height);
        self.compositor.veil_chain_mut().resize(&self.gpu.device, &self.gpu.queue, width, height);
        self.compositor.mark_needs_present();
    }

    // --- Undo / Redo ---

    pub fn undo(&mut self) {
        self.auto_commit_floating();
        let mut action = match self.undo_stack.pop_for_undo() {
            Some(a) => a,
            None => return,
        };

        let affected = action.undo(&mut self.doc);

        // If this is a GPU region action, execute the texture restore.
        if let Some(entry) = action.gpu_region_entry_mut() {
            let texture = if entry.format == wgpu::TextureFormat::R8Unorm {
                self.compositor.mask_texture(entry.layer_id).map(|t| &t.texture)
            } else {
                self.compositor.layer_texture(entry.layer_id).map(|t| &t.texture)
            };
            if let Some(texture) = texture {
                let mut encoder = self.gpu.device.create_command_encoder(
                    &wgpu::CommandEncoderDescriptor { label: Some("undo-restore") },
                );
                let forward = self.region_store.restore_region(&mut encoder, entry, texture);
                self.gpu.queue.submit([encoder.finish()]);
                *entry = forward;
            }
        }

        self.undo_stack.complete_undo(action);
        // Upload any CPU tiles that were restored by TileAction undo.
        for &layer_id in affected.keys() {
            self.upload_layer_tiles_to_gpu(layer_id);
        }
        self.sync_compositor_layers();
        self.compositor.mark_dirty();
        self.update_selection_overlay();
    }

    pub fn redo(&mut self) {
        self.auto_commit_floating();
        let mut action = match self.undo_stack.pop_for_redo() {
            Some(a) => a,
            None => return,
        };

        let affected = action.redo(&mut self.doc);

        // If this is a GPU region action, execute the texture restore (redo direction).
        if let Some(entry) = action.gpu_region_entry_mut() {
            let texture = if entry.format == wgpu::TextureFormat::R8Unorm {
                self.compositor.mask_texture(entry.layer_id).map(|t| &t.texture)
            } else {
                self.compositor.layer_texture(entry.layer_id).map(|t| &t.texture)
            };
            if let Some(texture) = texture {
                let mut encoder = self.gpu.device.create_command_encoder(
                    &wgpu::CommandEncoderDescriptor { label: Some("redo-restore") },
                );
                let backward = self.region_store.restore_region(&mut encoder, entry, texture);
                self.gpu.queue.submit([encoder.finish()]);
                *entry = backward;
            }
        }

        self.undo_stack.complete_redo(action);
        // Upload any CPU tiles that were restored by TileAction redo.
        for &layer_id in affected.keys() {
            self.upload_layer_tiles_to_gpu(layer_id);
        }
        self.sync_compositor_layers();
        self.compositor.mark_dirty();
        self.update_selection_overlay();
    }

    // --- Veils ---

    pub fn add_veil(&mut self, veil_type: &str, params: &[ParamValue]) {
        let chain = self.compositor.veil_chain_mut();
        let format = chain.accum_format();
        let veil = chain.registry_mut().create_veil(
            veil_type, params, &self.gpu.device, format,
        );
        chain.add_veil(&self.gpu.device, &self.gpu.queue, veil);
    }

    pub fn remove_veil(&mut self, index: usize) {
        self.compositor.veil_chain_mut().remove_veil(index);
    }

    pub fn clear_veils(&mut self) {
        self.compositor.veil_chain_mut().clear_veils();
    }

    pub fn set_veil_visible(&mut self, index: usize, visible: bool) {
        self.compositor.veil_chain_mut().set_veil_visible(index, visible);
    }

    pub fn move_veil(&mut self, from: usize, to: usize) {
        self.compositor.veil_chain_mut().move_veil(from, to);
    }

    pub fn update_veil(&mut self, index: usize, params: &[ParamValue]) {
        let type_id: &'static str = match self.compositor.veil_chain().type_id(index) {
            Some(t) => t,
            None => return,
        };
        let chain = self.compositor.veil_chain_mut();
        let format = chain.accum_format();
        let new_veil = chain.registry_mut().create_veil(
            type_id, params, &self.gpu.device, format,
        );
        chain.update_veil(&self.gpu.device, &self.gpu.queue, index, new_veil);
    }

    // --- Queries ---

    pub fn layer_tree(&self) -> Vec<LayerInfo> {
        self.doc.root.children.iter().rev().map(node_to_layer_info).collect()
    }

    pub fn veil_list(&self) -> Vec<VeilInfo> {
        let chain = self.compositor.veil_chain();
        let count = chain.count();
        let mut list = Vec::with_capacity(count);
        for i in (0..count).rev() {
            if let Some((type_id, visible)) = chain.info(i) {
                let param_defs = chain.registry().param_defs(type_id);
                let values = chain.param_values(i).unwrap_or_default();
                let params = param_defs.iter().enumerate().map(|(j, def)| {
                    ParamInfo::from_def(def, values.get(j))
                }).collect();
                list.push(VeilInfo {
                    type_id: type_id.to_string(),
                    visible,
                    index: i,
                    params,
                });
            }
        }
        list
    }

    /// Return all registered veil types with their parameter definitions.
    pub fn veil_types(&self) -> Vec<VeilTypeInfo> {
        self.compositor.veil_chain().registry().types()
            .into_iter()
            .map(|(type_id, defs)| VeilTypeInfo {
                type_id,
                params: defs.iter().map(|d| ParamInfo::from_def(d, None)).collect(),
            })
            .collect()
    }

    /// Get the parameter definitions for a veil type.
    pub fn veil_param_defs(&self, type_id: &str) -> &'static [ParamDef] {
        self.compositor.veil_chain().registry().param_defs(type_id)
    }

    // --- Tool Overlay ---

    pub fn set_overlay_primitives(&mut self, prims: Vec<OverlayPrimitive>) {
        self.tool_overlay = prims;
        self.push_merged_overlay();
    }

    pub fn clear_overlay(&mut self) {
        self.tool_overlay.clear();
        self.push_merged_overlay();
    }

    pub fn overlay_hit_test(&self, screen_x: f32, screen_y: f32) -> Option<usize> {
        self.compositor.overlay_hit_test(screen_x, screen_y)
    }

    /// Merge selection_overlay + tool_overlay and push to compositor.
    fn push_merged_overlay(&mut self) {
        let mut merged = Vec::with_capacity(self.selection_overlay.len() + self.tool_overlay.len());
        merged.extend_from_slice(&self.selection_overlay);
        merged.extend_from_slice(&self.tool_overlay);
        if merged.is_empty() {
            self.compositor.clear_overlay();
        } else {
            self.compositor.set_overlay_primitives(merged);
        }
    }

    // --- Selection ---

    pub fn select_rect(
        &mut self,
        x: f32, y: f32, w: f32, h: f32,
        mode: SelectionMode,
        antialias: bool,
        feather: f32,
    ) {
        let old_sel = self.doc.selection.clone();
        let mask = crate::tools::rect_select::rasterize(x, y, w, h, antialias, feather);
        self.doc.apply_selection(mask, mode);
        self.undo_stack.push(Box::new(SelectionAction::new(old_sel)));
        self.update_selection_overlay();
    }

    pub fn select_ellipse(
        &mut self,
        x: f32, y: f32, w: f32, h: f32,
        mode: SelectionMode,
        antialias: bool,
        feather: f32,
    ) {
        let old_sel = self.doc.selection.clone();
        let mask = crate::tools::ellipse_select::rasterize(x, y, w, h, antialias, feather);
        self.doc.apply_selection(mask, mode);
        self.undo_stack.push(Box::new(SelectionAction::new(old_sel)));
        self.update_selection_overlay();
    }

    pub fn select_lasso(
        &mut self,
        vertices: &[[f32; 2]],
        mode: SelectionMode,
        antialias: bool,
        feather: f32,
    ) {
        let old_sel = self.doc.selection.clone();
        let mask = crate::tools::lasso_select::rasterize(vertices, antialias, feather);
        self.doc.apply_selection(mask, mode);
        self.undo_stack.push(Box::new(SelectionAction::new(old_sel)));
        self.update_selection_overlay();
    }

    pub fn select_magic_wand(
        &mut self,
        layer_id: u64,
        seed_x: i32,
        seed_y: i32,
        tolerance: u8,
        mode: SelectionMode,
    ) {
        let source = match self.doc.layer(layer_id) {
            Some(Layer::Raster(r)) => &r.surface.store,
            _ => return,
        };
        let mask = crate::tools::magic_wand::rasterize(
            source,
            seed_x, seed_y,
            self.doc.width as i32, self.doc.height as i32,
            tolerance,
        );
        let old_sel = self.doc.selection.clone();
        self.doc.apply_selection(mask, mode);
        self.undo_stack.push(Box::new(SelectionAction::new(old_sel)));
        self.update_selection_overlay();
    }

    pub fn clear_selection(&mut self) {
        if self.doc.selection.is_none() {
            return;
        }
        let old_sel = self.doc.selection.clone();
        self.doc.selection = None;
        self.undo_stack.push(Box::new(SelectionAction::new(old_sel)));
        self.update_selection_overlay();
    }

    pub fn select_all(&mut self) {
        let old_sel = self.doc.selection.clone();
        let mask = crate::tools::rect_select::rasterize(
            0.0, 0.0, self.doc.width as f32, self.doc.height as f32, false, 0.0,
        );
        self.doc.selection = Some(mask);
        self.undo_stack.push(Box::new(SelectionAction::new(old_sel)));
        self.update_selection_overlay();
    }

    pub fn invert_selection(&mut self) {
        let old_sel = self.doc.selection.clone();
        if let Some(sel) = &mut self.doc.selection {
            sel.invert(self.doc.width, self.doc.height);
        }
        self.undo_stack.push(Box::new(SelectionAction::new(old_sel)));
        self.update_selection_overlay();
    }

    pub fn clear_selection_contents(&mut self, layer_id: u64) {
        self.auto_commit_floating();
        if self.doc.selection.is_none() {
            return;
        }
        self.gpu_clear_selection(layer_id);
    }

    pub fn has_selection(&self) -> bool {
        self.doc.selection.is_some()
    }

    // --- Copy / Cut / Paste ---

    /// Copy the active layer's content (masked by selection) into the internal
    /// clipboard. Kicks off an async GPU readback — the result is available via
    /// `poll_copy_result()` on the next frame. Returns `None` immediately.
    pub fn copy(&mut self, layer_id: u64) -> Option<ClipboardExport> {
        if self.doc.layer(layer_id).is_none() {
            return None;
        }

        self.start_copy_readback(layer_id, false);
        None
    }

    /// Poll for a completed copy result. Returns the ClipboardExport once the
    /// GPU readback has completed (typically the next frame after copy/cut).
    pub fn poll_copy_result(&mut self) -> Option<ClipboardExport> {
        self.pending_copy_result.take()
    }

    /// Start a GPU readback for copy (or cut). The readback completes
    /// asynchronously and is processed in `poll_pending`.
    fn start_copy_readback(&mut self, layer_id: u64, is_cut: bool) {
        let is_mask = self.editing_mask_layer == Some(layer_id);
        let canvas_w = self.doc.width;
        let canvas_h = self.doc.height;

        // Determine bounds and check texture exists.
        let format = if is_mask {
            if self.compositor.mask_texture(layer_id).is_none() { return; }
            wgpu::TextureFormat::R8Unorm
        } else {
            if self.compositor.layer_texture(layer_id).is_none() { return; }
            wgpu::TextureFormat::Rgba8Unorm
        };
        let region = self.copy_region_from_selection(canvas_w, canvas_h);

        let texture = if is_mask {
            &self.compositor.mask_texture(layer_id).unwrap().texture
        } else {
            &self.compositor.layer_texture(layer_id).unwrap().texture
        };

        let mut encoder = self.gpu.device.create_command_encoder(
            &wgpu::CommandEncoderDescriptor { label: Some("copy-readback") },
        );
        let mut request = readback::request_readback(
            &self.gpu.device, &mut encoder, texture, format, region,
        );
        self.gpu.queue.submit([encoder.finish()]);
        request.begin_mapping();

        // Also readback the selection mask for the same region if present.
        let selection_data = self.readback_selection_region(region);

        self.pending_copy = Some(PendingCopy {
            request,
            is_mask,
            region,
            selection_data,
            is_cut,
            layer_id,
        });
    }

    /// Determine the copy region from the selection (or full canvas).
    fn copy_region_from_selection(&self, canvas_w: u32, canvas_h: u32) -> [u32; 4] {
        if let Some(sel) = &self.doc.selection {
            if let Some((tx_min, ty_min, tx_max, ty_max)) = sel.bounding_rect() {
                let ts = TILE_SIZE as i32;
                let x = (tx_min * ts).max(0) as u32;
                let y = (ty_min * ts).max(0) as u32;
                let w = (((tx_max - tx_min + 1) * ts) as u32).min(canvas_w.saturating_sub(x));
                let h = (((ty_max - ty_min + 1) * ts) as u32).min(canvas_h.saturating_sub(y));
                return [x, y, w, h];
            }
        }
        [0, 0, canvas_w, canvas_h]
    }

    /// Read selection coverage for a given region from CPU-side AlphaMask.
    /// Returns None if there's no selection.
    fn readback_selection_region(&self, region: [u32; 4]) -> Option<Vec<u8>> {
        let selection = self.doc.selection.as_ref()?;
        let [rx, ry, rw, rh] = region;
        let ts = TILE_SIZE;
        let mut data = vec![0u8; (rw * rh) as usize];
        for ((tx, ty), tile) in selection.iter() {
            let base_x = tx * ts as i32;
            let base_y = ty * ts as i32;
            let td = tile.data();
            for ly in 0..ts {
                for lx in 0..ts {
                    let cx = base_x + lx as i32;
                    let cy = base_y + ly as i32;
                    let px = cx - rx as i32;
                    let py = cy - ry as i32;
                    if px >= 0 && py >= 0 && (px as u32) < rw && (py as u32) < rh {
                        let v = (td.get(lx, ly) * 255.0).clamp(0.0, 255.0) as u8;
                        data[(py as u32 * rw + px as u32) as usize] = v;
                    }
                }
            }
        }
        Some(data)
    }

    /// Cut = copy + clear. The clear happens after the readback completes.
    /// Returns `None` immediately; result available via `poll_copy_result()`.
    pub fn cut(&mut self, layer_id: u64) -> Option<ClipboardExport> {
        if self.doc.layer(layer_id).is_none() {
            return None;
        }
        self.start_copy_readback(layer_id, true);
        None
    }

    /// Paste raw RGBA bytes as a new layer. Used for both internal and external
    /// clipboard content. Returns the new layer ID.
    pub fn paste_image(
        &mut self,
        width: u32,
        height: u32,
        rgba: &[u8],
        offset_x: i32,
        offset_y: i32,
        active_layer_id: Option<u64>,
    ) -> u64 {
        // Create a new layer and insert above the active layer.
        let id = self.doc.add_raster_layer();
        if let Some(Layer::Raster(r)) = self.doc.layer_mut(id) {
            r.name = "Pasted Layer".to_string();
        }

        self.compositor.ensure_raster_layer(&self.gpu.device, &self.gpu.queue, id);

        // Write RGBA data directly to the GPU layer texture.
        let canvas_w = self.compositor.canvas_width();
        let canvas_h = self.compositor.canvas_height();

        // Clip the paste region to the canvas bounds.
        let src_x = (-offset_x).max(0) as u32;
        let src_y = (-offset_y).max(0) as u32;
        let dst_x = offset_x.max(0) as u32;
        let dst_y = offset_y.max(0) as u32;
        let copy_w = (width - src_x).min(canvas_w - dst_x);
        let copy_h = (height - src_y).min(canvas_h - dst_y);

        if copy_w > 0 && copy_h > 0 {
            if let Some(layer_tex) = self.compositor.layer_texture(id) {
                // Build a contiguous buffer for the clipped region.
                let row_bytes = copy_w as usize * 4;
                let mut buf = vec![0u8; row_bytes * copy_h as usize];
                for row in 0..copy_h as usize {
                    let src_row = (src_y as usize + row) * width as usize * 4 + src_x as usize * 4;
                    let dst_row = row * row_bytes;
                    buf[dst_row..dst_row + row_bytes]
                        .copy_from_slice(&rgba[src_row..src_row + row_bytes]);
                }

                self.gpu.queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &layer_tex.texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d { x: dst_x, y: dst_y, z: 0 },
                        aspect: wgpu::TextureAspect::All,
                    },
                    &buf,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(row_bytes as u32),
                        rows_per_image: None,
                    },
                    wgpu::Extent3d {
                        width: copy_w,
                        height: copy_h,
                        depth_or_array_layers: 1,
                    },
                );
            }
        }

        self.compositor.mark_dirty();

        // Position above active layer if specified.
        if let Some(active_id) = active_layer_id {
            self.doc.move_layer(id, MoveTarget::After(active_id));
        }

        let parent = self.doc.parent_of(id);
        let pos = self.doc.position_in_parent(id).unwrap_or(0);
        self.undo_stack.push(Box::new(LayerAddAction::new(id, parent, pos)));

        id
    }

    /// Paste from the internal clipboard at its original position.
    /// Returns the new layer ID, or None if clipboard is empty.
    pub fn paste_in_place(&mut self, active_layer_id: Option<u64>) -> Option<u64> {
        let clip = self.clipboard.as_ref()?.as_image()?;
        let (rgba, width, height, offset_x, offset_y) = clip.to_rgba();
        Some(self.paste_image(width, height, &rgba, offset_x, offset_y, active_layer_id))
    }

    // --- Floating Content (Phase 7) ---

    /// Auto-commit any active floating content before performing other edits.
    /// Call this before operations that would conflict with floating content
    /// (layer switch, paint, undo, etc.).
    pub fn auto_commit_floating(&mut self) {
        if self.floating.is_some() {
            self.commit_floating();
        }
    }

    /// Check if there is active floating content.
    pub fn has_floating(&self) -> bool {
        self.floating.is_some()
    }

    /// Return floating content info for the frontend overlay:
    /// (source_origin_x, source_origin_y, source_width, source_height, matrix[6]).
    /// Returns None if no floating content is active.
    pub fn floating_info(&self) -> Option<(f32, f32, f32, f32, Affine2D)> {
        self.floating.as_ref().map(|fc| (
            fc.source_origin.0 as f32,
            fc.source_origin.1 as f32,
            fc.source_width as f32,
            fc.source_height as f32,
            fc.matrix,
        ))
    }

    /// Paste from the internal clipboard as floating content on the current
    /// layer/mask. Returns true if floating content was created.
    pub fn paste_in_place_floating(&mut self, layer_id: u64) -> bool {
        // Auto-commit any existing floating content first.
        self.auto_commit_floating();

        let clip = match self.clipboard.as_ref().and_then(|c| c.as_image()) {
            Some(c) => c,
            None => return false,
        };

        let target_is_mask = self.editing_mask_layer == Some(layer_id);

        // Clone source tiles and dimensions from clipboard
        let (source_tiles, source_origin, source_width, source_height) = source_from_clip(clip);

        // Upload to GPU for preview
        self.compositor.set_floating_content(
            &self.gpu.device,
            &self.gpu.queue,
            &source_tiles,
            source_origin,
            source_width,
            source_height,
            layer_id,
            target_is_mask,
        );

        self.floating = Some(FloatingContent {
            source_origin,
            source_width,
            source_height,
            matrix: IDENTITY,
            target_layer: layer_id,
            target_is_mask,
            mode: FloatingMode::Paste,
        });

        true
    }

    /// Begin an interactive transform on the current layer/mask content.
    /// Returns true if floating content was created.
    pub fn begin_transform(&mut self, layer_id: u64) -> bool {
        self.auto_commit_floating();

        let target_is_mask = self.editing_mask_layer == Some(layer_id);

        if self.doc.layer(layer_id).is_none() {
            return false;
        }
        if target_is_mask {
            let has_mask = matches!(self.doc.layer(layer_id), Some(Layer::Raster(r)) if r.mask.is_some());
            if !has_mask { return false; }
        }

        let format = if target_is_mask {
            wgpu::TextureFormat::R8Unorm
        } else {
            wgpu::TextureFormat::Rgba8Unorm
        };
        let canvas_w = self.doc.width;
        let canvas_h = self.doc.height;

        // Determine source bounds.
        // With selection: use selection tile extent.
        // Without selection: use full canvas (content bounds tracking is a future optimization).
        let (source_origin, source_width, source_height) = if let Some(sel) = &self.doc.selection {
            match sel.bounding_rect() {
                Some((tx_min, ty_min, tx_max, ty_max)) => {
                    let ts = TILE_SIZE as i32;
                    let x = tx_min * ts;
                    let y = ty_min * ts;
                    let w = ((tx_max - tx_min + 1) * ts) as u32;
                    let h = ((ty_max - ty_min + 1) * ts) as u32;
                    // Clamp to canvas bounds.
                    let x = x.max(0);
                    let y = y.max(0);
                    let w = w.min(canvas_w.saturating_sub(x as u32));
                    let h = h.min(canvas_h.saturating_sub(y as u32));
                    ((x, y), w, h)
                }
                None => return false, // empty selection
            }
        } else {
            ((0i32, 0i32), canvas_w, canvas_h)
        };

        if source_width == 0 || source_height == 0 {
            return false;
        }

        // Save the full canvas GPU texture to scratch (pre-clear snapshot for
        // undo and cancel). Must happen before the clear.
        {
            let texture = if target_is_mask {
                self.compositor.mask_texture(layer_id).map(|t| &t.texture)
            } else {
                self.compositor.layer_texture(layer_id).map(|t| &t.texture)
            };
            if let Some(texture) = texture {
                let mut encoder = self.gpu.device.create_command_encoder(
                    &wgpu::CommandEncoderDescriptor { label: Some("transform-save") },
                );
                self.region_store.save_region(
                    &mut encoder, texture, format,
                    [0, 0, canvas_w, canvas_h],
                );
                self.gpu.queue.submit([encoder.finish()]);
            }
        }

        // Copy source region from GPU texture to transform source texture.
        {
            let mut encoder = self.gpu.device.create_command_encoder(
                &wgpu::CommandEncoderDescriptor { label: Some("transform-copy-source") },
            );
            self.compositor.set_floating_content_from_gpu(
                &self.gpu.device,
                &self.gpu.queue,
                &mut encoder,
                source_origin,
                source_width,
                source_height,
                layer_id,
                target_is_mask,
            );
            self.gpu.queue.submit([encoder.finish()]);
        }

        // If selection is active, mask the source texture so only selected pixels
        // are included in the transform. Also clear only selected pixels on the layer.
        let has_selection = self.doc.selection.is_some();
        if has_selection {
            // Upload a cropped selection mask matching the source region dimensions.
            let cropped_sel_bg = self.upload_cropped_selection_mask(
                source_origin, source_width, source_height,
            );
            // Full-canvas selection for erasing on the layer.
            let full_sel_bg = self.upload_selection_mask(canvas_w, canvas_h);

            if let Some(sel_bg) = &cropped_sel_bg {
                // Multiply source texture by selection mask — zeroes out unselected pixels.
                if let Some(source_tex) = self.compositor.transform_source_texture() {
                    let target = GpuPaintTarget {
                        texture: source_tex.0,
                        view: source_tex.1,
                        format,
                        width: source_width,
                        height: source_height,
                    };
                    let mut encoder = self.gpu.device.create_command_encoder(
                        &wgpu::CommandEncoderDescriptor { label: Some("transform-sel-mask") },
                    );
                    target.multiply_by_mask(
                        &mut encoder, &self.paint_pipelines, &self.gpu.queue, sel_bg,
                    );
                    self.gpu.queue.submit([encoder.finish()]);
                }
            }

            if let Some(sel_bg) = &full_sel_bg {
                // Clear selected pixels on the layer using erase_with_selection.
                let layer_target = if target_is_mask {
                    self.compositor.mask_texture(layer_id)
                        .map(|t| GpuPaintTarget::from_mask(t, canvas_w, canvas_h))
                } else {
                    self.compositor.layer_texture(layer_id)
                        .map(|t| GpuPaintTarget::from_layer(t, canvas_w, canvas_h))
                };
                if let Some(target) = layer_target {
                    let mut encoder = self.gpu.device.create_command_encoder(
                        &wgpu::CommandEncoderDescriptor { label: Some("transform-clear-sel") },
                    );
                    target.erase_with_selection(
                        &mut encoder, &self.paint_pipelines, &self.gpu.queue, sel_bg,
                    );
                    self.gpu.queue.submit([encoder.finish()]);
                }
            }
        } else {
            // No selection — clear the full source region on the layer.
            let clear_x = source_origin.0.max(0) as u32;
            let clear_y = source_origin.1.max(0) as u32;
            let clear_w = source_width.min(canvas_w.saturating_sub(clear_x));
            let clear_h = source_height.min(canvas_h.saturating_sub(clear_y));
            let clear_rect = [clear_x, clear_y, clear_w, clear_h];

            let target = if target_is_mask {
                self.compositor.mask_texture(layer_id)
                    .map(|t| GpuPaintTarget::from_mask(t, canvas_w, canvas_h))
            } else {
                self.compositor.layer_texture(layer_id)
                    .map(|t| GpuPaintTarget::from_layer(t, canvas_w, canvas_h))
            };
            if let Some(target) = target {
                let mut encoder = self.gpu.device.create_command_encoder(
                    &wgpu::CommandEncoderDescriptor { label: Some("transform-clear") },
                );
                target.clear_rect(
                    &mut encoder, &self.paint_pipelines, &self.gpu.queue,
                    clear_rect,
                );
                self.gpu.queue.submit([encoder.finish()]);
            }
        }

        let clear_x = source_origin.0.max(0) as u32;
        let clear_y = source_origin.1.max(0) as u32;
        let clear_w = source_width.min(canvas_w.saturating_sub(clear_x));
        let clear_h = source_height.min(canvas_h.saturating_sub(clear_y));
        let clear_rect = [clear_x, clear_y, clear_w, clear_h];

        self.floating = Some(FloatingContent {
            source_origin,
            source_width,
            source_height,
            matrix: IDENTITY,
            target_layer: layer_id,
            target_is_mask,
            mode: FloatingMode::Transform { format, clear_rect },
        });

        // Selection was used to define what gets picked up — clear it now so
        // the marching ants disappear and the transform output isn't clipped.
        if has_selection {
            self.doc.selection = None;
            self.update_selection_overlay();
        }

        true
    }

    /// Update the floating content's transform matrix.
    pub fn update_floating_matrix(&mut self, matrix: Affine2D) {
        if let Some(fc) = &mut self.floating {
            fc.matrix = matrix;
            self.compositor.update_floating_matrix(
                &self.gpu.queue,
                &matrix,
                fc.source_origin,
                fc.source_width,
                fc.source_height,
            );
        }
    }

    /// Commit floating content: render transformed pixels into the target
    /// layer/mask texture via a GPU render pass.
    pub fn commit_floating(&mut self) {
        let fc = match self.floating.take() {
            Some(fc) => fc,
            None => return,
        };

        let layer_id = fc.target_layer;
        let is_mask = fc.target_is_mask;
        let format = if is_mask {
            wgpu::TextureFormat::R8Unorm
        } else {
            wgpu::TextureFormat::Rgba8Unorm
        };

        // Compute tight affected rect = union(source bounds, transformed bounds),
        // clamped to canvas.
        let canvas_w = self.doc.width;
        let canvas_h = self.doc.height;
        let (min_x, min_y, max_x, max_y) = fc.transformed_bounds();
        let (sox, soy) = fc.source_origin;
        let union_min_x = min_x.min(sox).max(0) as u32;
        let union_min_y = min_y.min(soy).max(0) as u32;
        let union_max_x = (max_x.max(sox + fc.source_width as i32) as u32).min(canvas_w);
        let union_max_y = (max_y.max(soy + fc.source_height as i32) as u32).min(canvas_h);
        let affected_w = union_max_x.saturating_sub(union_min_x);
        let affected_h = union_max_y.saturating_sub(union_min_y);
        let affected_rect = [union_min_x, union_min_y, affected_w, affected_h];

        // For paste mode, the scratch doesn't have a pre-operation snapshot yet
        // (begin_transform wasn't called). Save the current state now.
        if matches!(fc.mode, FloatingMode::Paste) {
            let texture = if is_mask {
                self.compositor.mask_texture(layer_id).map(|t| &t.texture)
            } else {
                self.compositor.layer_texture(layer_id).map(|t| &t.texture)
            };
            if let Some(texture) = texture {
                let mut encoder = self.gpu.device.create_command_encoder(
                    &wgpu::CommandEncoderDescriptor { label: Some("paste-save") },
                );
                self.region_store.save_region(
                    &mut encoder, texture, format,
                    [0, 0, canvas_w, canvas_h],
                );
                self.gpu.queue.submit([encoder.finish()]);
            }
        }

        // Commit the pre-operation state (from scratch) to the undo ring buffer,
        // then render the transform.
        let mut encoder = self.gpu.device.create_command_encoder(
            &wgpu::CommandEncoderDescriptor { label: Some("transform-commit") },
        );
        let entry = self.region_store.commit_region(
            &mut encoder, layer_id, format, affected_rect,
        );

        // GPU render pass: write transformed source pixels to layer/mask texture.
        self.compositor.commit_floating_to_texture(
            &mut encoder, &self.gpu.queue,
            &fc.matrix, fc.source_origin, fc.source_width, fc.source_height,
        );

        self.gpu.queue.submit([encoder.finish()]);

        // Push GPU undo action.
        self.undo_stack.push(Box::new(GpuRegionAction::new(entry)));

        // Clean up GPU state
        self.compositor.clear_floating_content();
    }

    /// Cancel floating content: discard or restore original pixels.
    pub fn cancel_floating(&mut self) {
        let fc = match self.floating.take() {
            Some(fc) => fc,
            None => return,
        };

        match fc.mode {
            FloatingMode::Paste => {
                // No-op — target layer was never modified.
            }
            FloatingMode::Transform { format, clear_rect } => {
                // Restore the pre-clear state from the RegionStore scratch
                // texture (saved during begin_transform).
                let texture = if fc.target_is_mask {
                    self.compositor.mask_texture(fc.target_layer).map(|t| &t.texture)
                } else {
                    self.compositor.layer_texture(fc.target_layer).map(|t| &t.texture)
                };
                if let Some(texture) = texture {
                    let mut encoder = self.gpu.device.create_command_encoder(
                        &wgpu::CommandEncoderDescriptor { label: Some("cancel-restore") },
                    );
                    self.region_store.restore_from_scratch(
                        &mut encoder, format, clear_rect, texture,
                    );
                    self.gpu.queue.submit([encoder.finish()]);
                }
            }
        }

        self.compositor.clear_floating_content();
    }

    /// Regenerate marching ants overlay from the current selection.
    fn update_selection_overlay(&mut self) {
        self.selection_overlay.clear();

        if let Some(sel) = &self.doc.selection {
            let segments = sel.contour_segments(0.5);
            for (a, b) in &segments {
                // Black background line (slightly thicker, solid)
                let mut bg = OverlayPrimitive::new(
                    KIND_DASHED_LINE,
                    FLAG_CANVAS_SPACE,
                    *a, *b,
                );
                bg.color = [0.0, 0.0, 0.0, 1.0];
                bg.thickness = 1.5;
                bg.dash_len = 0.0; // solid
                self.selection_overlay.push(bg);
            }
            for (a, b) in &segments {
                // White foreground dashes
                let mut fg = OverlayPrimitive::new(
                    KIND_DASHED_LINE,
                    FLAG_CANVAS_SPACE,
                    *a, *b,
                );
                fg.color = [1.0, 1.0, 1.0, 1.0];
                fg.thickness = 1.0;
                fg.dash_len = 8.0;
                self.selection_overlay.push(fg);
            }
        }

        self.push_merged_overlay();
    }

    // --- Internal helpers ---

    fn sync_compositor_layers(&mut self) {
        // Collect raster layer info first to avoid borrow conflicts with mask_dirty.
        struct RasterInfo {
            id: u64,
            opacity: f32,
            blend_mode: BlendMode,
            show_mask: bool,
            mask_enabled: bool,
            has_mask: bool,
        }
        let infos: Vec<RasterInfo> = self.doc.all_raster_layers().into_iter().map(|r| {
            RasterInfo {
                id: r.id, opacity: r.opacity, blend_mode: r.blend_mode,
                show_mask: r.show_mask, mask_enabled: r.mask_enabled,
                has_mask: r.mask.is_some(),
            }
        }).collect();

        for info in &infos {
            self.compositor.ensure_raster_layer(&self.gpu.device, &self.gpu.queue, info.id);
            self.compositor.update_raster_uniforms_full(
                &self.gpu.queue, info.id, info.opacity, info.blend_mode, info.show_mask,
            );
            self.compositor.set_layer_mask(&self.gpu.device, &self.gpu.queue, info.id, info.has_mask);
            self.compositor.update_mask_binding(
                &self.gpu.device, info.id, info.mask_enabled, info.show_mask,
            );
            // Upload mask tiles to GPU after undo/redo (dirty upload loop removed).
            if info.has_mask {
                self.upload_mask_to_gpu(info.id);
            }
        }

        // Sync non-passthrough group state
        let groups: Vec<(u64, f32, BlendMode, bool)> = self.doc.all_groups()
            .iter()
            .filter(|g| !g.passthrough)
            .map(|g| (g.id, g.opacity, g.blend_mode, g.show_mask))
            .collect();
        for (id, opacity, blend_mode, show_mask) in groups {
            self.compositor.ensure_group_state(&self.gpu.device, &self.gpu.queue, id);
            self.compositor.update_group_uniforms(&self.gpu.queue, id, opacity, blend_mode, show_mask);
        }
    }
}

fn node_to_layer_info(node: &LayerNode) -> LayerInfo {
    match node {
        LayerNode::Layer(layer) => match layer {
            Layer::Raster(r) => LayerInfo::Raster {
                id: r.id as f64,
                name: r.name.clone(),
                visible: r.visible,
                opacity: r.opacity,
                blend_mode: r.blend_mode as u32,
                has_mask: r.mask.is_some(),
                mask_enabled: r.mask_enabled,
                show_mask: r.show_mask,
            },
        },
        LayerNode::Group(g) => LayerInfo::Group {
            id: g.id as f64,
            name: g.name.clone(),
            visible: g.visible,
            collapsed: g.collapsed,
            passthrough: g.passthrough,
            opacity: g.opacity,
            blend_mode: g.blend_mode as u32,
            has_mask: g.mask.is_some(),
            mask_enabled: g.mask_enabled,
            show_mask: g.show_mask,
            children: g.children.iter().rev().map(node_to_layer_info).collect(),
        },
    }
}

// ---------------------------------------------------------------------------
// Thumbnail generation — CPU-side nearest-neighbor sampling from tile data
// ---------------------------------------------------------------------------

fn generate_rgba_thumbnail(
    tiles: &TileGrid,
    doc_w: u32, doc_h: u32,
    thumb_w: u32, thumb_h: u32,
) -> Vec<u8> {
    let mut buf = vec![0u8; (thumb_w * thumb_h * 4) as usize];
    let ts = TILE_SIZE as u32;

    for oy in 0..thumb_h {
        let cy = (oy * doc_h / thumb_h).min(doc_h - 1);
        let ty = (cy / ts) as i32;
        let ly = (cy % ts) as usize;

        for ox in 0..thumb_w {
            let cx = (ox * doc_w / thumb_w).min(doc_w - 1);
            let tx = (cx / ts) as i32;
            let lx = (cx % ts) as usize;

            let off = ((oy * thumb_w + ox) * 4) as usize;

            let (r, g, b, a) = if let Some(tile) = tiles.get(tx, ty) {
                let p = tile.data().pixel(lx, ly);
                (p[0], p[1], p[2], p[3])
            } else {
                (0, 0, 0, 0)
            };

            // Checkerboard behind transparent areas
            let check = if ((ox / 4) + (oy / 4)) % 2 == 0 { 102u8 } else { 153u8 };
            let af = a as f32 / 255.0;
            buf[off]     = (r as f32 * af + check as f32 * (1.0 - af)) as u8;
            buf[off + 1] = (g as f32 * af + check as f32 * (1.0 - af)) as u8;
            buf[off + 2] = (b as f32 * af + check as f32 * (1.0 - af)) as u8;
            buf[off + 3] = 255;
        }
    }
    buf
}

fn generate_mask_thumbnail(
    mask: &AlphaMask,
    doc_w: u32, doc_h: u32,
    thumb_w: u32, thumb_h: u32,
) -> Vec<u8> {
    let mut buf = vec![0u8; (thumb_w * thumb_h * 4) as usize];
    let ts = TILE_SIZE as u32;

    for oy in 0..thumb_h {
        let cy = (oy * doc_h / thumb_h).min(doc_h - 1);
        let ty = (cy / ts) as i32;
        let ly = (cy % ts) as usize;

        for ox in 0..thumb_w {
            let cx = (ox * doc_w / thumb_w).min(doc_w - 1);
            let tx = (cx / ts) as i32;
            let lx = (cx % ts) as usize;

            let v = if let Some(tile) = mask.get(tx, ty) {
                (tile.data().get(lx, ly) * 255.0) as u8
            } else {
                255 // no tile = white (reveal all) — matches get_or_create_full() default
            };

            let off = ((oy * thumb_w + ox) * 4) as usize;
            buf[off]     = v;
            buf[off + 1] = v;
            buf[off + 2] = v;
            buf[off + 3] = 255;
        }
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::params::{ParamDef, ParamValue};

    #[test]
    fn param_info_serializes_flat() {
        let def = ParamDef::Float { name: "speed", min: 0.0, max: 10.0, default: 1.0 };
        let info = ParamInfo::from_def(&def, Some(&ParamValue::Float(2.5)));
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["kind"], "float");
        assert_eq!(json["name"], "speed");
        assert_eq!(json["min"], 0.0);
        assert_eq!(json["max"], 10.0);
        assert_eq!(json["default"], 1.0);
        assert_eq!(json["value"], 2.5);
    }

    #[test]
    fn param_info_bool_omits_min_max() {
        let def = ParamDef::Bool { name: "soft", default: true };
        let info = ParamInfo::from_def(&def, None);
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["kind"], "bool");
        assert_eq!(json["name"], "soft");
        assert_eq!(json["default"], true);
        assert!(json.get("min").is_none());
        assert!(json.get("max").is_none());
        assert!(json.get("value").is_none());
    }

    #[test]
    fn veil_info_serializes_correctly() {
        let info = VeilInfo {
            type_id: "pixelate".into(),
            visible: true,
            index: 0,
            params: vec![
                ParamInfo::from_def(
                    &ParamDef::Int { name: "scale", min: 1, max: 32, default: 2 },
                    Some(&ParamValue::Int(4)),
                ),
                ParamInfo::from_def(
                    &ParamDef::Bool { name: "soft", default: true },
                    Some(&ParamValue::Bool(false)),
                ),
            ],
        };
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["type"], "pixelate");
        assert_eq!(json["visible"], true);
        assert_eq!(json["index"], 0);

        let params = json["params"].as_array().unwrap();
        assert_eq!(params.len(), 2);
        assert_eq!(params[0]["kind"], "int");
        assert_eq!(params[0]["name"], "scale");
        assert_eq!(params[0]["value"], 4);
        assert_eq!(params[1]["kind"], "bool");
        assert_eq!(params[1]["name"], "soft");
        assert_eq!(params[1]["value"], false);
    }
}

