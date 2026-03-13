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

#[test]
fn all_wgsl_shaders_compile() {
    let shader_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../shaders");
    let shader_dir = shader_dir.canonicalize().expect("shaders/ directory not found");
    let files = find_wgsl_files(&shader_dir);
    assert!(!files.is_empty(), "no .wgsl files found in {shader_dir:?}");

    let mut failures = Vec::new();
    for path in &files {
        let source = std::fs::read_to_string(path).unwrap();
        let result = naga::front::wgsl::parse_str(&source);
        if let Err(e) = result {
            let name = path.strip_prefix(&shader_dir).unwrap_or(path);
            failures.push(format!("{}: {e}", name.display()));
        }
    }

    if !failures.is_empty() {
        panic!(
            "{} shader(s) failed to compile:\n\n{}",
            failures.len(),
            failures.join("\n\n")
        );
    }

    eprintln!("validated {} WGSL shaders", files.len());
}
