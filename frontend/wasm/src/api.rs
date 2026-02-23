use darkly::document::Document;
use darkly::layer::Layer;
use darkly::undo::{UndoStack, mark_affected_dirty};
use darkly::gpu::compositor::Compositor;
use darkly::gpu::context::GpuContext;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct DarklyHandle {
    doc: Document,
    compositor: Compositor,
    gpu: GpuContext,
    undo_stack: UndoStack,
    /// The layer currently being painted (transaction active).
    active_transaction_layer: Option<u64>,
}

#[wasm_bindgen]
impl DarklyHandle {
    /// Create a new Darkly editor instance, initializing GPU and document.
    pub async fn create(canvas: web_sys::HtmlCanvasElement) -> DarklyHandle {
        let width = canvas.width();
        let height = canvas.height();

        let gpu = GpuContext::new(canvas).await;
        let compositor = Compositor::new(&gpu.device, gpu.surface_format(), width, height);
        let doc = Document::new(width, height);
        let undo_stack = UndoStack::new(50);

        DarklyHandle {
            doc,
            compositor,
            gpu,
            undo_stack,
            active_transaction_layer: None,
        }
    }

    /// Add a new raster layer and return its ID.
    pub fn add_raster_layer(&mut self) -> u64 {
        let id = self.doc.add_raster_layer();
        self.compositor.ensure_raster_layer(&self.gpu.device, &self.gpu.queue, id);
        self.compositor.mark_dirty();
        id
    }

    /// Add a filter layer. `filter_type` is the filter type string (e.g., "noise").
    /// `params` is a JS object with filter-specific parameters.
    pub fn add_filter_layer(&mut self, filter_type: &str, params: JsValue) -> u64 {
        let format = self.compositor.accum_format();
        let filter = darkly::gpu::filter::create_filter(
            filter_type,
            params,
            &self.gpu.device,
            format,
            self.compositor.filter_pipelines_mut(),
        );

        let id = self.doc.add_filter_layer(filter.clone_boxed());

        if let Some(Layer::Filter(f)) = self.doc.layer(id) {
            self.compositor.ensure_filter_layer(
                &self.gpu.device,
                &self.gpu.queue,
                id,
                f.filter.as_ref(),
            );
        }

        self.compositor.mark_dirty();
        id
    }

    /// Paint a circle on a raster layer.
    pub fn paint(
        &mut self,
        layer_id: u64,
        x: f32,
        y: f32,
        radius: f32,
        r: u8,
        g: u8,
        b: u8,
        a: u8,
    ) {
        self.doc.paint_circle(layer_id, x, y, radius, [r, g, b, a]);
    }

    /// Fill a raster layer with a demo gradient.
    pub fn fill_gradient(&mut self, layer_id: u64) {
        self.doc.fill_gradient(layer_id);
    }

    /// Render the current frame. Handles dirty checking internally (P2).
    pub fn render(&mut self) {
        self.compositor.render(
            &self.gpu.device,
            &self.gpu.queue,
            &self.gpu.surface,
            &self.gpu.surface_config,
            &mut self.doc,
        );
    }

    /// Set opacity for a layer.
    pub fn set_opacity(&mut self, layer_id: u64, opacity: f32) {
        if let Some(Layer::Raster(r)) = self.doc.layer_mut(layer_id) {
            r.opacity = opacity;
            self.compositor.update_raster_uniforms(
                &self.gpu.queue,
                layer_id,
                opacity,
                r.blend_mode,
            );
            self.compositor.mark_dirty();
        }
    }

    /// Set blend mode for a layer.
    pub fn set_blend_mode(&mut self, layer_id: u64, mode: u32) {
        if let Some(Layer::Raster(r)) = self.doc.layer_mut(layer_id) {
            let blend_mode = darkly::layer::BlendMode::from_u32(mode);
            r.blend_mode = blend_mode;
            self.compositor
                .update_raster_uniforms(&self.gpu.queue, layer_id, r.opacity, blend_mode);
            self.compositor.mark_dirty();
        }
    }

    /// Begin a stroke: start recording tile changes on the given layer.
    /// Called on mousedown. Replaces the old `snapshot()` method.
    pub fn snapshot(&mut self, layer_id: u64) {
        self.doc.begin_transaction(layer_id);
        self.active_transaction_layer = Some(layer_id);
    }

    /// End a stroke: commit the transaction and push to undo stack.
    /// Called on mouseup.
    pub fn commit(&mut self) {
        if let Some(layer_id) = self.active_transaction_layer.take() {
            if let Some(step) = self.doc.commit_transaction(layer_id) {
                self.undo_stack.push(step);
            }
        }
    }

    /// Undo the last stroke.
    pub fn undo(&mut self) {
        if let Some(affected) = self.undo_stack.undo(&mut self.doc) {
            mark_affected_dirty(&mut self.doc.dirty, &affected);
            self.compositor.mark_dirty();
        }
    }

    /// Redo the last undone stroke.
    pub fn redo(&mut self) {
        if let Some(affected) = self.undo_stack.redo(&mut self.doc) {
            mark_affected_dirty(&mut self.doc.dirty, &affected);
            self.compositor.mark_dirty();
        }
    }

    /// Resize the canvas.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.gpu.resize(width, height);
    }
}
