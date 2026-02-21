use wasm_bindgen::prelude::*;

use darkly_core::document::Document;
use darkly_core::layer::{FilterParams, FilterType, Layer};
use darkly_core::undo::UndoStack;
use darkly_gpu::compositor::Compositor;
use darkly_gpu::context::GpuContext;

#[wasm_bindgen]
pub struct DarklyHandle {
    doc: Document,
    compositor: Compositor,
    gpu: GpuContext,
    undo_stack: UndoStack,
}

#[wasm_bindgen]
impl DarklyHandle {
    #[wasm_bindgen(constructor)]
    pub async fn create(canvas: web_sys::HtmlCanvasElement) -> Result<DarklyHandle, JsValue> {
        let width = canvas.width();
        let height = canvas.height();
        log::info!("Creating DarklyHandle for canvas {width}x{height}");

        let gpu = GpuContext::new(canvas).await;
        let compositor = Compositor::new(&gpu.device, gpu.surface_format(), width, height);
        let doc = Document::new(width, height);
        let undo_stack = UndoStack::new(50);

        Ok(DarklyHandle {
            doc,
            compositor,
            gpu,
            undo_stack,
        })
    }

    /// Check if anything needs rendering (P2).
    /// Called by the rAF loop to skip render() when idle.
    pub fn needs_render(&self) -> bool {
        self.compositor.needs_render(&self.doc)
    }

    pub fn render(&mut self) {
        self.compositor.render(
            &self.gpu.device,
            &self.gpu.queue,
            &self.gpu.surface,
            &self.gpu.surface_config,
            &mut self.doc,
        );
    }

    pub fn add_raster_layer(&mut self) -> u64 {
        let id = self.doc.add_raster_layer();
        self.compositor.mark_dirty();
        id
    }

    pub fn add_filter_layer(&mut self, filter_type: u32, param: f32) -> u64 {
        let ft = match filter_type {
            0 => FilterType::GaussianBlur,
            _ => FilterType::GaussianBlur,
        };
        let params = FilterParams::blur(param);
        let id = self.doc.add_filter_layer(ft, params);
        self.compositor.mark_dirty();
        id
    }

    pub fn fill_gradient(&mut self, layer_id: u64) {
        self.doc.fill_gradient(layer_id);
        // Dirty tiles are marked by fill_gradient; compositor will detect them
        // during tile upload and set needs_composite automatically.
    }

    pub fn paint(&mut self, layer_id: u64, x: f32, y: f32, radius: f32, r: u8, g: u8, b: u8, a: u8) {
        self.doc.paint_circle(layer_id, x, y, radius, [r, g, b, a]);
        // Dirty tiles are marked by paint_circle; compositor will detect them
        // during tile upload and set needs_composite automatically.
    }

    pub fn set_opacity(&mut self, layer_id: u64, opacity: f32) {
        if let Some(layer) = self.doc.layer_mut(layer_id) {
            match layer {
                Layer::Raster(r) => r.opacity = opacity,
                _ => {}
            }
        }
        self.compositor.mark_dirty();
    }

    pub fn set_blend_mode(&mut self, layer_id: u64, mode: u32) {
        use darkly_core::layer::BlendMode;
        if let Some(Layer::Raster(r)) = self.doc.layer_mut(layer_id) {
            r.blend_mode = match mode {
                0 => BlendMode::Normal,
                1 => BlendMode::Multiply,
                2 => BlendMode::Screen,
                3 => BlendMode::Overlay,
                _ => BlendMode::Normal,
            };
        }
        self.compositor.mark_dirty();
    }

    pub fn snapshot(&mut self) {
        self.undo_stack.push(&self.doc);
    }

    pub fn undo(&mut self) {
        self.undo_stack.undo(&mut self.doc);
        self.compositor.mark_dirty();
    }

    pub fn redo(&mut self) {
        self.undo_stack.redo(&mut self.doc);
        self.compositor.mark_dirty();
    }
}
