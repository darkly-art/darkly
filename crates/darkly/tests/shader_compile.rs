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
    let shader_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("shaders");
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

        // Skip preamble-only files, since they have no entry points and are
        // validated indirectly when prepended to the shaders that use them.
        let has_entry_point = source.contains("@vertex")
            || source.contains("@fragment")
            || source.contains("@compute");
        if !has_entry_point {
            continue;
        }

        // Prepend any preamble whose symbols are referenced by this shader.
        // A preamble that exports multiple helpers (e.g. `lib/fbm.wgsl` ships
        // `fbm`, `fbm_warp`, `fbm_warp_offset`, etc.) might be referenced by
        // any of its names; collect them all and match against the union.
        let mut full_source = String::new();
        for (_, preamble_src) in &preambles {
            let fn_names: Vec<&str> = preamble_src
                .lines()
                .filter_map(|line| {
                    let line = line.trim();
                    if line.starts_with("fn ") {
                        line.strip_prefix("fn ")?.split('(').next()
                    } else {
                        None
                    }
                })
                .collect();
            if fn_names.iter().any(|n| source.contains(n)) {
                full_source.push_str(preamble_src);
                full_source.push('\n');
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

/// Split the top-level (paren-depth-0) comma-separated arguments of a call,
/// starting just after the opening `(`. Returns the argument substrings and
/// the index of the matching closing `)`.
fn split_call_args(src: &str, open_paren: usize) -> (Vec<String>, usize) {
    let bytes = src.as_bytes();
    let mut depth = 1i32;
    let mut args = Vec::new();
    let mut start = open_paren + 1;
    let mut i = start;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    args.push(src[start..i].trim().to_string());
                    return (args, i);
                }
            }
            b',' if depth == 1 => {
                args.push(src[start..i].trim().to_string());
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    (args, bytes.len())
}

/// Parse a WGSL numeric literal (`0.4`, `-0.5`, `1.0f`, `2`, `.3h`) to f64,
/// or `None` if the text is anything other than a bare numeric literal (e.g.
/// an expression like `0.2 * r` or a variable).
fn parse_wgsl_number(s: &str) -> Option<f64> {
    let s = s.trim();
    // Strip the WGSL float/int type suffix if present.
    let s = s
        .strip_suffix('f')
        .or_else(|| s.strip_suffix('h'))
        .unwrap_or(s);
    if s.is_empty() {
        return None;
    }
    s.parse::<f64>().ok()
}

/// Regression: no shader may call `smoothstep(low, high, x)` with **constant**
/// edges where `low >= high` (a reversed-edge constant smoothstep).
///
/// naga (the native/Firefox front-end, used by `all_wgsl_shaders_compile`) is
/// lenient and accepts it, but the browser's WGSL validator (Tint) const-
/// evaluates the edges and rejects `low >= high` as a hard compile error,
/// invalidating the whole shader module → the render pipeline → every command
/// buffer that binds it. This is what broke all the veils (rainy_glass, vhs)
/// in the shipped desktop build: they compiled in dev (naga) but every
/// `*-pipeline` came back invalid under the AppImage's Chromium/Dawn.
///
/// The fix is the algebraic identity `smoothstep(hi, lo, x)` ≡
/// `1.0 - smoothstep(lo, hi, x)` (smoothstep is symmetric about its midpoint),
/// which is bit-identical on every platform and conformant everywhere.
///
/// Only *constant* edges are checked: Tint only rejects what it can const-
/// evaluate, so a runtime-reversed `smoothstep(0.2 * r, 0.0, x)` compiles fine
/// (it computes the well-defined clamp formula) and is intentionally allowed.
#[test]
fn no_reversed_edge_constant_smoothstep() {
    let shader_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("shaders");
    let files = find_wgsl_files(&shader_dir);
    let mut violations = Vec::new();

    for path in &files {
        let source = std::fs::read_to_string(path).unwrap();
        let name = path.strip_prefix(&shader_dir).unwrap_or(path);
        for (idx, _) in source.match_indices("smoothstep") {
            // Only a real call: the next non-space char after the name is `(`.
            let after = &source[idx + "smoothstep".len()..];
            let paren_rel = match after.find(|c: char| !c.is_whitespace()) {
                Some(p) if after.as_bytes()[p] == b'(' => idx + "smoothstep".len() + p,
                _ => continue,
            };
            let (args, _) = split_call_args(&source, paren_rel);
            if args.len() < 2 {
                continue;
            }
            let (Some(lo), Some(hi)) = (parse_wgsl_number(&args[0]), parse_wgsl_number(&args[1]))
            else {
                continue; // one or both edges are runtime expressions (allowed)
            };
            if lo >= hi {
                let line = source[..idx].bytes().filter(|&b| b == b'\n').count() + 1;
                violations.push(format!(
                    "{}:{line}: smoothstep({lo}, {hi}, …) has low >= high: \
                     rewrite as 1.0 - smoothstep({hi}, {lo}, …)",
                    name.display()
                ));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "reversed-edge constant smoothstep is rejected by the browser's WGSL \
         validator (Tint) and breaks the shader in packaged builds:\n\n{}",
        violations.join("\n")
    );
}
