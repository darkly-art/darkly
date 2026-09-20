//! Temporary: prints the centreline rows that `brush_accumulation.rs` pins.
//! Deleted once the pins are stored.
use darkly::engine::types::StrokeOp;
use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::{test_adapter_name, test_device};

const W: u32 = 256;
const H: u32 = 128;

#[test]
fn print_pins() {
    eprintln!("ADAPTER {}", test_adapter_name());
    for name in ["Pencil", "Build-up Pencil", "Ink Pen"] {
        let (device, queue) = test_device();
        let mut engine = DarklyEngine::new(GpuContext::new_headless(device, queue), W, H);
        let layer = engine.add_raster_layer(None);
        let brush = darkly::brush::builtin_brushes::all()
            .into_iter()
            .find(|b| b.metadata.name == name)
            .unwrap();
        let json = serde_json::to_string(&brush.metadata.graph).unwrap();
        engine.set_brush_graph(&json).unwrap();
        engine.begin_stroke(layer).unwrap();
        let mut t = 0.0f64;
        for i in 0..40u32 {
            engine.stroke_to(StrokeOp::BrushStroke {
                x: 8.0 + i as f32 * ((W as f32 - 16.0) / 40.0),
                y: (H / 2) as f32,
                pressure: 0.7,
                x_tilt: 0.0,
                y_tilt: 0.0,
                rotation: 0.0,
                tangential_pressure: 0.0,
                time_ms: t,
                cr: 0.0,
                cg: 0.0,
                cb: 0.0,
                ca: 1.0,
            });
            t += 16.0;
        }
        engine.end_stroke();
        engine.test_flush_readbacks();
        let px = engine.test_readback_layer(layer);
        let y = H / 2;
        let row: Vec<u8> = (0..W)
            .map(|x| px[(((y * W + x) * 4) + 3) as usize])
            .collect();
        eprintln!("ROW {name} {row:?}");
    }
}
