mod common;
use common::{new_engine, ConfigGuard};
use darkly::engine::types::StrokeOp;

fn op(x: f32, y: f32, pressure: f32, t: f64) -> StrokeOp {
    StrokeOp::BrushStroke { x, y, pressure, x_tilt: 0.0, y_tilt: 0.0, rotation: 0.0, tangential_pressure: 0.0, time_ms: t }
}

fn run(name: &str, stabilize: f32, horizon: f64, ops: &[StrokeOp], every: usize, canvas: (u32, u32)) {
    let _p = ConfigGuard::set("input.predictionHorizon", horizon);
    let mut engine = new_engine(canvas, "Ink Pen", stabilize);
    engine.set_stroke_seed(Some(7));
    let layer = engine.add_raster_layer(None);
    engine.begin_stroke(layer).unwrap();
    for (i, op) in ops.iter().enumerate() {
        engine.stroke_to(*op);
        if i % every == 0 { engine.render(0.0); }
    }
    engine.end_stroke();
    println!("MEASURE {name} s={stabilize} pred={horizon} every={every}: copies {} rewinds {} dabs {} fallbacks {}",
        engine.test_stroke_checkpoint_full_copies(), engine.test_stroke_rewinds(), engine.test_stroke_total_dabs(), engine.test_stroke_full_rerender_events());
}

#[test]
fn measure() {
    let tap: Vec<StrokeOp> = [0.1, 0.3, 0.6, 0.85, 0.9, 0.8, 0.5, 0.25, 0.1].into_iter().enumerate().map(|(i, q)| op(500.0, 500.0, q, i as f64 * 4.5)).collect();
    let hatch: Vec<StrokeOp> = (0..36).map(|i| { let u = i as f32 / 35.0; op(400.0 + i as f32 * 3.0, 400.0 + i as f32 * 2.0, (std::f32::consts::PI * u).sin().max(0.05), i as f64 * 1000.0 / 240.0) }).collect();
    let curve: Vec<StrokeOp> = (0..900).map(|i| { let th = i as f32 * 0.01; let r = 380.0 * (1.0 - th * 0.05); let left = (900 - i) as f32;
        let q = if left < 30.0 { left / 30.0 } else { 0.5 + 0.4 * (th * 3.0).sin() }; op(512.0 + r * th.cos(), 512.0 + r * th.sin(), q, i as f64 * 1000.0 / 240.0) }).collect();
    let wavy: Vec<StrokeOp> = (0..900).map(|i| { let t = i as f64 * 1000.0 / 240.0; op(40.0 + i as f32 * 1.05, 512.0, 0.5 + 0.4 * (2.0 * std::f32::consts::PI * t as f32 / 200.0).sin(), t) }).collect();
    for s in [0.0f32, 0.4, 1.0] {
        run("tap", s, 0.0, &tap, 1, (1024, 1024));
        run("tap60", s, 0.0, &tap, 4, (1024, 1024));
        run("hatch", s, 0.0, &hatch, 4, (1024, 1024));
    }
    for (s, h) in [(0.0f32, 0.0), (0.4, 10.0), (0.6, 0.0), (1.0, 30.0)] {
        run("curve", s, h, &curve, 4, (1024, 1024));
    }
    for s in [0.4f32, 0.6, 1.0] {
        run("wavy", s, 0.0, &wavy, 4, (1024, 1024));
    }
}
