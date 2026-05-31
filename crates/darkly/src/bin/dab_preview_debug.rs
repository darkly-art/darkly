//! Visual debug for the dab-preview renderer.
//!
//! Renders one or more builtin brushes' dab through the real engine,
//! frames each one via `frame_dab_thumbnail`, and dumps both the framed
//! PNG (`<brush>-framed.png`) and the unframed 256×256 raw render
//! (`<brush>-raw.png`) under `./debug-output/dab-preview/`. Generates an
//! `index.html` that shows all brushes side by side so you can compare.
//!
//! Usage:
//!     cargo run --bin dab_preview_debug --features testing
//!     cargo run --bin dab_preview_debug --features testing -- Charcoal "Ink Pen" Round

use std::fs;
use std::path::Path;

use darkly::engine::rendering::frame_dab_thumbnail;
use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::test_device;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let brush_names: Vec<String> = if args.is_empty() {
        vec!["Charcoal".into(), "Ink Pen".into()]
    } else {
        args
    };

    let out_dir = Path::new("debug-output/dab-preview");
    fs::create_dir_all(out_dir).expect("create out dir");

    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    let mut engine = DarklyEngine::new(gpu, 1024, 768);

    let bg = engine.debug_preview_theme_bg();
    println!("preview bg = ({:.3}, {:.3}, {:.3})", bg[0], bg[1], bg[2]);

    let mut figures: Vec<String> = Vec::new();
    for name in &brush_names {
        let Some((raw, w, h)) = engine.debug_brush_dab_raw_pixels(name) else {
            eprintln!("brush '{name}' not found in library — skipping");
            continue;
        };

        let safe = sanitize(name);
        let raw_path = out_dir.join(format!("{safe}-raw.png"));
        let framed_path = out_dir.join(format!("{safe}-framed.png"));

        let stats = compute_stats(&raw, w, h);
        println!("{name}:");
        println!(
            "  raw 256×256: min={:.0} mean={:.2} max={:.0}",
            stats.min, stats.mean, stats.max
        );

        let raw_img = image::RgbaImage::from_raw(w, h, raw.clone()).expect("raw -> RgbaImage");
        raw_img.save(&raw_path).expect("save raw");

        let framed = frame_dab_thumbnail(&raw, w, h, bg);
        fs::write(&framed_path, &framed).expect("write framed");

        let framed_mean = decoded_mean_luminance(&framed);
        println!("  framed mean luminance = {framed_mean:.2}");

        figures.push(format!(
            r#"  <figure>
    <div class="thumbs">
      <div class="on-black"><img class="framed" src="{safe}-framed.png" alt="{name_escaped} framed"></div>
      <div class="on-white"><img class="framed" src="{safe}-framed.png" alt="{name_escaped} framed"></div>
      <img class="raw" src="{safe}-raw.png" alt="{name_escaped} raw 256×256">
    </div>
    <figcaption><strong>{name_escaped}</strong><br>raw min={min:.0} mean={mean:.2} max={max:.0}<br>framed mean luminance {framed_mean:.2}</figcaption>
  </figure>"#,
            safe = safe,
            name_escaped = html_escape(name),
            min = stats.min,
            mean = stats.mean,
            max = stats.max,
            framed_mean = framed_mean,
        ));
    }

    let html = format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>dab preview debug</title>
<style>
  body {{
    background: #111;
    color: #ddd;
    font: 14px/1.4 system-ui, sans-serif;
    margin: 24px;
  }}
  h1 {{ font-weight: 500; }}
  .row {{
    display: flex;
    flex-wrap: wrap;
    gap: 48px;
    margin-top: 24px;
  }}
  figure {{ margin: 0; text-align: center; }}
  figcaption {{ margin-top: 8px; color: #999; font-size: 13px; }}
  .thumbs {{ display: flex; gap: 12px; align-items: flex-start; justify-content: center; }}
  .thumbs > div {{ padding: 8px; border-radius: 4px; }}
  .on-black {{ background: #000; }}
  .on-white {{ background: #fff; }}
  img {{
    image-rendering: pixelated;
    display: block;
    border: 1px solid #333;
  }}
  .framed {{ width: 96px; height: 96px; }}
  .raw {{ width: 192px; height: 192px; background: #000; }}
</style>
</head>
<body>
<h1>dab preview — brush comparison</h1>
<p>Each row shows the framed PNG on black and white panels, plus the raw
256×256 render on the right. Stats are computed on the raw render.</p>
<div class="row">
{figures}
</div>
</body>
</html>
"#,
        figures = figures.join("\n"),
    );
    fs::write(out_dir.join("index.html"), html).expect("write index.html");

    println!();
    println!(
        "open: file://{}/index.html",
        out_dir.canonicalize().unwrap().display()
    );
}

struct Stats {
    min: f64,
    mean: f64,
    max: f64,
}

fn compute_stats(pixels: &[u8], w: u32, h: u32) -> Stats {
    let mut sum = 0.0_f64;
    let mut min_l = 255.0_f64;
    let mut max_l = 0.0_f64;
    let n = (w * h) as usize;
    for chunk in pixels.chunks_exact(4) {
        let r = chunk[0] as f64;
        let g = chunk[1] as f64;
        let b = chunk[2] as f64;
        let l = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        sum += l;
        if l < min_l {
            min_l = l;
        }
        if l > max_l {
            max_l = l;
        }
    }
    Stats {
        min: min_l,
        mean: sum / n as f64,
        max: max_l,
    }
}

fn decoded_mean_luminance(png: &[u8]) -> f64 {
    let img = image::load_from_memory(png).expect("valid PNG");
    let rgba = img.to_rgba8();
    let mut sum = 0.0_f64;
    for p in rgba.pixels() {
        sum += 0.2126 * p.0[0] as f64 + 0.7152 * p.0[1] as f64 + 0.0722 * p.0[2] as f64;
    }
    sum / (rgba.width() * rgba.height()) as f64
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
