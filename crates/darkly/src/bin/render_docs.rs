//! Writes an animated preview (a PNG frame sequence) for every previewable
//! registry entry, plus a small JSON index of what it wrote.
//!
//! ```text
//! cargo run -p darkly --bin render_docs --features testing -- --out <dir>
//! ```
//!
//! `--stills --catalog <id>` writes one JPEG poster per entry instead, into this
//! repository's own preview directory: the images the generated markdown tables
//! embed. That mode is run by hand when a catalog gains or loses an entry; the
//! sequence mode above is what the release workflow runs.
//!
//! Kept separate from `export-docs` because that one is GPU-free by
//! construction: folding both into one binary would drag the metadata export
//! behind a GPU device and the `testing` feature it does not need.
//!
//! Everything except argument handling and reporting lives in
//! [`darkly::docs_render`], which the integration tests call directly;
//! coverage tooling runs test targets and never executes a `[[bin]]`.

use std::process::ExitCode;

use darkly::docs_render::{self, Command};

fn main() -> ExitCode {
    let command = match docs_render::parse_args(std::env::args().skip(1)) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("render_docs: {e}\n\n{}", docs_render::USAGE);
            return ExitCode::FAILURE;
        }
    };
    let out = match command {
        Command::Help => {
            print!("{}", docs_render::USAGE);
            return ExitCode::SUCCESS;
        }
        Command::Stills { out, catalog } => {
            return match docs_render::render_stills(&out, &catalog) {
                Ok(written) => {
                    for path in &written {
                        println!("{}", path.display());
                    }
                    println!("{catalog}: {} stills", written.len());
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("render_docs: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Command::Frames { out } => out,
    };

    match docs_render::render_all(&out) {
        Ok(manifest) => {
            for (catalog, entries) in &manifest.assets {
                let frames: u32 = entries.values().map(|a| a.frames).sum();
                // Every entry in a catalog is rendered by one renderer, so one
                // entry's size describes the whole catalog.
                let size = entries
                    .values()
                    .next()
                    .map(|a| format!("{} × {}", a.width, a.height))
                    .unwrap_or_default();
                println!(
                    "{catalog}: {} assets, {frames} frames, {size}",
                    entries.len()
                );
            }
            println!("{} - version {}", out.display(), manifest.version);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("render_docs: {e}");
            ExitCode::FAILURE
        }
    }
}
