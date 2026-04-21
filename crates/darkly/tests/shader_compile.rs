use std::path::{Path, PathBuf};

fn find_wgsl_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            files.extend(find_wgsl_files(&path));
        } else if path.extension().is_some_and(|e| e == "wgsl") {
            files.push(path);
        }
    }
    files.sort();
    files
}

/// Find preamble files: .wgsl files that define helper functions but have no
/// entry points (@vertex / @fragment / @compute). These are concatenated onto
/// shaders that reference their symbols, mirroring the `concat!(include_str!())`
/// pattern used in production Rust code.
fn load_preambles(files: &[PathBuf]) -> Vec<(PathBuf, String)> {
    let mut preambles = Vec::new();
    for path in files {
        let source = std::fs::read_to_string(path).unwrap();
        let has_entry_point = source.contains("@vertex")
            || source.contains("@fragment")
            || source.contains("@compute");
        if !has_entry_point {
            preambles.push((path.clone(), source));
        }
    }
    preambles
}

#[test]
fn all_wgsl_shaders_compile() {
    let shader_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../shaders");
    let shader_dir = shader_dir
        .canonicalize()
        .expect("shaders/ directory not found");
    let files = find_wgsl_files(&shader_dir);
    assert!(!files.is_empty(), "no .wgsl files found in {shader_dir:?}");

    let preambles = load_preambles(&files);

    let mut failures = Vec::new();
    let mut validated = 0;
    for path in &files {
        let source = std::fs::read_to_string(path).unwrap();

        // Skip preamble-only files — they have no entry points and are
        // validated indirectly when prepended to the shaders that use them.
        let has_entry_point = source.contains("@vertex")
            || source.contains("@fragment")
            || source.contains("@compute");
        if !has_entry_point {
            continue;
        }

        // Prepend any preamble whose symbols are referenced by this shader.
        let mut full_source = String::new();
        for (_, preamble_src) in &preambles {
            // Extract the function name from the preamble (first `fn <name>` line).
            if let Some(fn_name) = preamble_src.lines().find_map(|line| {
                let line = line.trim();
                if line.starts_with("fn ") {
                    line.strip_prefix("fn ")?.split('(').next()
                } else {
                    None
                }
            }) {
                if source.contains(fn_name) {
                    full_source.push_str(preamble_src);
                    full_source.push('\n');
                }
            }
        }
        full_source.push_str(&source);

        let result = naga::front::wgsl::parse_str(&full_source);
        if let Err(e) = result {
            let name = path.strip_prefix(&shader_dir).unwrap_or(path);
            failures.push(format!("{}: {e}", name.display()));
        }
        validated += 1;
    }

    if !failures.is_empty() {
        panic!(
            "{} shader(s) failed to compile:\n\n{}",
            failures.len(),
            failures.join("\n\n")
        );
    }

    eprintln!(
        "validated {validated} WGSL shaders ({} preambles skipped)",
        preambles.len()
    );
}
