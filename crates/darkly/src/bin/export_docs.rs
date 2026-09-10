//! Write one JSON file describing everything Darkly registers.
//!
//! The file is self-describing: a consumer needs no Darkly source, no shared
//! helpers, no knowledge of Darkly's hotkey resolution, and no particular
//! renderer. Everything requiring Darkly knowledge to compute (layered preset
//! resolution, chord rendering for both platform conventions, unit-suffixed
//! display strings) is computed here.
//!
//! Needs no GPU: every registry constructor is pure, so the catalogs build from
//! `&'static` registration data alone.
//!
//! ```text
//! cargo run -p darkly --bin export-docs -- --out /tmp/darkly-docs/metadata.json
//! ```

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

use darkly::catalog::{catalogs, settings_catalogs, Catalog};
use darkly::config::{preset_bindings, Binding, OVERLAYS};

/// The whole artifact. `schema` is the shape's own version: bumped when a
/// consumer would need to change to keep reading it.
#[derive(serde::Serialize)]
struct DocsManifest {
    schema: u32,
    /// `git describe` of the build that wrote this, which is also what
    /// `render-docs` stamps: the pairing key between the two artifacts.
    version: &'static str,
    catalogs: Vec<Catalog>,
    /// Preset name → action id → the chords it resolves to. Keyed by the
    /// preset's own name, with `defaults` for the editor-agnostic baseline.
    bindings: BTreeMap<String, BTreeMap<String, Vec<Binding>>>,
}

const SCHEMA_VERSION: u32 = 1;

const HELP: &str = "\
export-docs - write Darkly's registry metadata to a JSON file

USAGE:
    export-docs --out <path>

OPTIONS:
    --out <path>    File to write. Parent directories are created.
    -h, --help      Show this message.
";

fn parse_args() -> Result<PathBuf, String> {
    let mut out: Option<PathBuf> = None;
    let mut argv = std::env::args().skip(1);
    while let Some(a) = argv.next() {
        match a.as_str() {
            "--out" => {
                let v = argv.next().ok_or("--out needs a path")?;
                out = Some(PathBuf::from(v));
            }
            "-h" | "--help" => {
                print!("{HELP}");
                std::process::exit(0);
            }
            other => return Err(format!("unrecognized argument `{other}`")),
        }
    }
    out.ok_or_else(|| "--out is required".to_string())
}

fn main() -> ExitCode {
    let out = match parse_args() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("export-docs: {e}\n\n{HELP}");
            return ExitCode::FAILURE;
        }
    };

    let mut all = catalogs();
    all.extend(settings_catalogs());

    let mut bindings = BTreeMap::new();
    bindings.insert("defaults".to_string(), preset_bindings(None));
    for (name, _) in OVERLAYS {
        bindings.insert((*name).to_string(), preset_bindings(Some(name)));
    }

    let entries: usize = all.iter().map(|c| c.entries.len()).sum();
    let manifest = DocsManifest {
        schema: SCHEMA_VERSION,
        version: darkly::VERSION,
        catalogs: all,
        bindings,
    };

    let json = match serde_json::to_string_pretty(&manifest) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("export-docs: failed to serialize: {e}");
            return ExitCode::FAILURE;
        }
    };

    if let Some(parent) = out.parent() {
        if !parent.as_os_str().is_empty() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                eprintln!("export-docs: cannot create {}: {e}", parent.display());
                return ExitCode::FAILURE;
            }
        }
    }
    if let Err(e) = std::fs::write(&out, &json) {
        eprintln!("export-docs: cannot write {}: {e}", out.display());
        return ExitCode::FAILURE;
    }

    println!(
        "{} - {} catalogs, {} entries, {} presets, {} KB",
        out.display(),
        manifest.catalogs.len(),
        entries,
        manifest.bindings.len(),
        json.len() / 1024,
    );
    ExitCode::SUCCESS
}
