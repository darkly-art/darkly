//! Re-render every generated region in the repository's markdown.
//!
//! ```text
//! cargo sync-docs              # rewrite
//! cargo sync-docs -- --check   # report only
//! ```
//!
//! `--check` is what `tests/docs_md.rs` asserts and what CI therefore enforces;
//! the writing mode is what you run by hand to make a stale checkout correct.
//! Needs no GPU — every fragment builds from `&'static` registration data, the
//! same property that lets the check live in the ordinary test suite.

use std::path::PathBuf;
use std::process::ExitCode;

use darkly::docs_md::{self, Mode};

const HELP: &str = "\
sync-docs — fill the generated regions of the repository's markdown

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

    let report = match docs_md::sync(&args.root, args.mode) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("sync-docs: {e}");
            return ExitCode::FAILURE;
        }
    };

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
