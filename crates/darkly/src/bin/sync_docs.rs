//! Re-render every generated region in the repository, and the generated
//! product metadata beside them.
//!
//! ```text
//! cargo sync-docs              # rewrite
//! cargo sync-docs -- --check   # report only
//! ```
//!
//! `--check` is what `tests/docs_md.rs` asserts and what CI therefore enforces;
//! the writing mode is what you run by hand to make a stale checkout correct.
//! Needs no GPU and reads no files: every fragment builds from the registries
//! and from text embedded at compile time, the same property that lets the
//! check live in the ordinary test suite.

use std::path::PathBuf;
use std::process::ExitCode;

use darkly::docs_md::{self, Mode};
use darkly::product;

const HELP: &str = "\
sync-docs - fill the generated regions of the repository's markdown

USAGE:
    sync-docs [--check] [--root <path>]

OPTIONS:
    --check         Report out-of-date files and write nothing. Exits non-zero
                    if any region is stale.
    --root <path>   Repository root. Defaults to this crate's own checkout.
    -h, --help      Show this message.
";

struct Args {
    mode: Mode,
    root: PathBuf,
}

fn parse_args() -> Result<Args, String> {
    let mut mode = Mode::Write;
    let mut root = None;
    let mut argv = std::env::args().skip(1);
    while let Some(a) = argv.next() {
        match a.as_str() {
            "--check" => mode = Mode::Check,
            "--root" => root = Some(PathBuf::from(argv.next().ok_or("--root needs a path")?)),
            "-h" | "--help" => {
                print!("{HELP}");
                std::process::exit(0);
            }
            other => return Err(format!("unrecognized argument `{other}`")),
        }
    }
    Ok(Args {
        mode,
        root: root.unwrap_or_else(docs_md::repo_root),
    })
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("sync-docs: {e}\n\n{HELP}");
            return ExitCode::FAILURE;
        }
    };

    let mut report = match docs_md::sync(&args.root, args.mode) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("sync-docs: {e}");
            return ExitCode::FAILURE;
        }
    };

    // The machine-readable projection of the same product metadata, for
    // consumers that cannot read a Rust constant. Not a region, because JSON
    // has no comment syntax to hide a marker in: the whole file is generated.
    match product::sync_app_json(&args.root, args.mode == Mode::Write) {
        Ok(stale) => {
            let path = PathBuf::from(product::APP_JSON);
            report.generated.push(path.clone());
            if stale {
                report.changed.push(path);
            }
        }
        Err(e) => {
            eprintln!("sync-docs: {}: {e}", product::APP_JSON);
            return ExitCode::FAILURE;
        }
    }

    if report.changed.is_empty() {
        println!(
            "{} generated {} up to date",
            report.generated.len(),
            if report.generated.len() == 1 {
                "file"
            } else {
                "files"
            }
        );
        return ExitCode::SUCCESS;
    }

    for file in &report.changed {
        println!(
            "{} {}",
            if args.mode == Mode::Check {
                "stale:"
            } else {
                "wrote:"
            },
            file.display()
        );
    }
    match args.mode {
        Mode::Check => {
            eprintln!("sync-docs: run `cargo run -p darkly --bin sync-docs` to update");
            ExitCode::FAILURE
        }
        Mode::Write => ExitCode::SUCCESS,
    }
}
