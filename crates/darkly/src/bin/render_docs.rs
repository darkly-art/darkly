//! Writes an animated preview — a PNG frame sequence — for every previewable
//! registry entry, plus a small JSON index of what it wrote.
//!
//! ```text
//! cargo run -p darkly --bin render_docs --features testing -- --out <dir>
//! ```
//!
//! Kept separate from `export-docs` because that one is GPU-free by
//! construction: folding both into one binary would drag the metadata export
//! behind a GPU device and the `testing` feature it does not need.
//!
//! Everything except argument handling and reporting lives in
//! [`darkly::docs_render`], which the integration tests call directly —
//! coverage tooling runs test targets and never executes a `[[bin]]`.

use std::process::ExitCode;

use darkly::docs_render::{self, Args};

fn main() -> ExitCode {
    let args = match docs_render::parse_args(std::env::args().skip(1)) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("render_docs: {e}\n\n{}", docs_render::USAGE);
            return ExitCode::FAILURE;
        }
    };
    let Args { out: Some(out) } = args else {
        print!("{}", docs_render::USAGE);
        return ExitCode::SUCCESS;
    };

    match docs_render::render_all(&out) {
        Ok(manifest) => {
            for (catalog, entries) in &manifest.assets {
                let frames: u32 = entries.values().map(|a| a.frames).sum();
                println!("{catalog}: {} assets, {frames} frames", entries.len());
            }
            println!(
                "{} — {} × {}, version {}",
                out.display(),
                manifest.width,
                manifest.height,
                manifest.version
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("render_docs: {e}");
            ExitCode::FAILURE
        }
    }
}
