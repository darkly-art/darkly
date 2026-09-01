use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Bake Darkly's version into the crate as the `DARKLY_VERSION` compile-time
/// env, read through `crate::VERSION`. The value is the latest git tag plus the
/// commit height since it (`git describe --tags --long`, e.g. `v0.3.0-1-gf0c3ea9`)
/// — the same v* tags the deploy pipeline (darkly-deploy/) releases from.
///
/// CANONICAL TWIN: frontend/vite.config.ts derives the frontend's version with
/// the identical command and the identical `"0.0.0-0-gunknown"` fallback. The
/// two build systems (Cargo vs. Vite) share no runtime, so this is a documented
/// DRY exception — if you change the command or fallback here, change it there.
///
/// Note: baking the commit SHA makes the crate's output non-deterministic across
/// commits (as is already true for the frontend bundle). Best-effort and never
/// panics — a tagless/git-less build just gets the fallback.
fn emit_darkly_version() {
    // No `--always`: on a tagless/shallow checkout we want this to FAIL so the
    // fallback kicks in, rather than emit a bare SHA that isn't `TAG-N-gSHA`.
    let version = Command::new("git")
        .args(["describe", "--tags", "--long"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "0.0.0-0-gunknown".to_string());

    println!("cargo:rustc-env=DARKLY_VERSION={version}");

    // Re-stamp when git state moves — best-effort and footgun-free: emit a hint
    // ONLY for a path that exists, because a hint pointing at a missing file
    // makes cargo treat it as perpetually-changed (rebuild every time). These
    // hints only reduce dev staleness; release correctness comes from the
    // deploy pipeline's fresh clone-at-tag, not from here. Tag-at-HEAD and
    // `git gc` repacks are imperfectly covered by design.
    let git_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("../../.git");
    let mut candidates = vec![
        git_dir.join("HEAD"),
        git_dir.join("packed-refs"),
        git_dir.join("refs/tags"),
    ];
    // If HEAD is a symref (`ref: refs/heads/x`), watch the loose branch ref too.
    if let Ok(head) = fs::read_to_string(git_dir.join("HEAD")) {
        if let Some(r) = head.trim().strip_prefix("ref: ") {
            candidates.push(git_dir.join(r));
        }
    }
    for path in candidates {
        if path.exists() {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
}

fn main() {
    emit_darkly_version();

    let src = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("src");

    generate_grouped_registry(
        &src.join("engine/protocol/handlers"),
        "crate::engine::protocol::RequestRegistration",
    );

    generate_handler_registry(&src.join("engine"));

    // Registries whose variants are browsable metadata. `catalog_sources` is
    // what `crate::catalog` is generated from — see `generate_catalog_registry`.
    let mut catalog_sources: Vec<(String, String)> = Vec::new();

    generate_catalog_registry(
        &src.join("actions"),
        "crate::action::ActionCategory",
        &src,
        &mut catalog_sources,
    );

    generate_catalog_registry(
        &src.join("gpu/effects"),
        "crate::gpu::effect::EffectRegistration",
        &src,
        &mut catalog_sources,
    );

    generate_catalog_registry(
        &src.join("gpu/voids"),
        "crate::gpu::void::VoidRegistration",
        &src,
        &mut catalog_sources,
    );

    generate_catalog_registry(
        &src.join("tools"),
        "crate::tool::ToolRegistration",
        &src,
        &mut catalog_sources,
    );

    generate_catalog_registry(
        &src.join("brush/nodes"),
        "crate::brush::BrushNodeRegistration",
        &src,
        &mut catalog_sources,
    );

    generate_registry(
        &src.join("brush/stabilizers"),
        "crate::brush::stabilizer::StabilizerRegistration",
    );

    generate_registry(
        &src.join("config/sections"),
        "crate::config::schema::SchemaSection",
    );

    generate_catalog_registry(
        &src.join("document/filters"),
        "crate::document::filter::FilterEntityRegistration",
        &src,
        &mut catalog_sources,
    );

    generate_catalog_registry(
        &src.join("document/layer_kinds"),
        "crate::document::layer_kind::LayerKindRegistration",
        &src,
        &mut catalog_sources,
    );

    generate_catalog_registry(
        &src.join("gpu/blend_modes"),
        "crate::gpu::blend_mode::BlendModeRegistration",
        &src,
        &mut catalog_sources,
    );

    // Brushes are a directory of YAML data rather than of `register()` modules,
    // so they record their catalog source from inside their own scan. Must run
    // before `generate_catalog_sources`, which consumes the vector.
    generate_builtin_brushes(
        &PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("brushes"),
        &mut catalog_sources,
    );

    generate_catalog_sources(catalog_sources, &src);

    generate_yaml_presets(&PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("presets"));

    generate_texture_registry(
        &PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("resources/textures"),
    );
}

/// [`generate_registry`], additionally recording the directory as a source of
/// browsable catalog metadata.
///
/// Which function a registry directory is scanned by *is* the decision about
/// whether its variants are documentation. Both `catalogs()` and
/// `catalog_sources()` are generated from what this records, so a registry
/// cannot be projected in one and forgotten in the other — and a directory
/// scanned by plain [`generate_registry`] (brush nodes, stabilizers, request
/// handler groups) contributes to neither.
///
/// The registry module — the parent module of the registration type — must
/// export `CATALOG_ID` and `catalog()`, and may export `preview_mechanism()`.
fn generate_catalog_registry(
    dir: &Path,
    registration_type: &str,
    src: &Path,
    sources: &mut Vec<(String, String)>,
) {
    generate_registry(dir, registration_type);
    let rel = dir
        .strip_prefix(src)
        .unwrap_or(dir)
        .to_str()
        .unwrap()
        .replace('\\', "/");
    record_catalog_source(
        &rel,
        registration_type.rsplit_once("::").unwrap().0,
        sources,
    );
}

/// Record a scanned directory as a source of browsable catalog metadata.
///
/// Split out from [`generate_catalog_registry`] for the scans whose directory
/// holds data rather than `register()` modules — `brushes/` is a directory of
/// YAML, but its catalog is documentation on the same footing as a registry's.
/// `module` must export `CATALOG_ID` and `catalog()`, and may export a preview
/// mechanism.
fn record_catalog_source(dir: &str, module: &str, sources: &mut Vec<(String, String)>) {
    sources.push((dir.to_string(), module.to_string()));
}

/// Resolve a module path (`crate::a::b`) to the file that holds it, trying
/// `src/a/b.rs` then `src/a/b/mod.rs`. `None` when neither exists.
fn module_source(module: &str, src: &Path) -> Option<PathBuf> {
    let rel = module.trim_start_matches("crate::").replace("::", "/");
    let flat = src.join(format!("{rel}.rs"));
    if flat.exists() {
        return Some(flat);
    }
    let dir = src.join(&rel).join("mod.rs");
    dir.exists().then_some(dir)
}

/// Emit `OUT_DIR/catalog_sources_gen.rs`: the list of scanned catalog-producing
/// registry directories, the `catalogs()` projection over them, and the
/// `preview_mechanisms()` projection over the subset that has one. Generated
/// rather than hand-written so the export and the test that checks the export
/// is complete both read from what the build actually found on disk.
fn generate_catalog_sources(mut sources: Vec<(String, String)>, src: &Path) {
    sources.sort();

    let mut code = String::new();
    code.push_str("// @generated by build.rs — do not edit manually.\n");
    code.push_str(
        "// One entry per registry directory scanned by `generate_catalog_registry`.\n\n",
    );

    code.push_str("/// A registry directory the build scan found to produce a catalog.\n");
    code.push_str("pub struct CatalogSource {\n");
    code.push_str("    /// The directory the build scan walked, as that scan named it:\n");
    code.push_str("    /// `gpu/effects` and friends relative to `crates/darkly/src`,\n");
    code.push_str("    /// `brushes` beside it.\n");
    code.push_str("    pub dir: &'static str,\n");
    code.push_str("    /// Id of the catalog the registry in that directory produces.\n");
    code.push_str("    pub id: &'static str,\n");
    code.push_str("}\n\n");

    code.push_str("/// Every module directory the build scan found that produces a catalog.\n");
    code.push_str("#[rustfmt::skip]\n");
    code.push_str("pub fn catalog_sources() -> Vec<CatalogSource> {\n");
    code.push_str("    vec![\n");
    for (dir, module) in &sources {
        code.push_str(&format!(
            "        CatalogSource {{ dir: \"{dir}\", id: {module}::CATALOG_ID }},\n"
        ));
    }
    code.push_str("    ]\n");
    code.push_str("}\n\n");

    code.push_str("/// Every registry, projected. Requires no GPU device.\n");
    code.push_str("#[rustfmt::skip]\n");
    code.push_str("pub fn catalogs() -> Vec<Catalog> {\n");
    code.push_str("    vec![\n");
    for (_, module) in &sources {
        code.push_str(&format!("        {module}::catalog(),\n"));
    }
    code.push_str("    ]\n");
    code.push_str("}\n\n");

    // One row per catalog whose registry module exports a preview mechanism.
    // A catalog that has none writes nothing — which is what keeps the
    // document-layer registries free of a `wgpu`-taking trait rather than
    // making each of them hand-write a negative.
    code.push_str(
        "/// Every catalog that can render a preview, keyed by catalog id. A catalog\n\
         /// whose registry module exports no `preview_mechanism` is absent, which is\n\
         /// how a non-previewable catalog answers without writing anything.\n",
    );
    code.push_str("#[rustfmt::skip]\n");
    code.push_str(
        "pub fn preview_mechanisms() -> Vec<(&'static str, &'static dyn crate::gpu::preview::PreviewMechanism)> {\n",
    );
    code.push_str("    vec![\n");
    for (_, module) in &sources {
        let Some(path) = module_source(module, src) else {
            continue;
        };
        println!("cargo:rerun-if-changed={}", path.display());
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        if text.contains("pub fn preview_mechanism") {
            code.push_str(&format!(
                "        ({module}::CATALOG_ID, {module}::preview_mechanism()),\n"
            ));
        }
    }
    code.push_str("    ]\n");
    code.push_str("}\n");

    let out_dir = env::var("OUT_DIR").expect("OUT_DIR not set");
    let out_path = PathBuf::from(out_dir).join("catalog_sources_gen.rs");
    fs::write(&out_path, code).unwrap();
}

/// Scan a directory for .rs module files (excluding mod.rs) and generate
/// a mod.rs that re-exports all modules and provides a `registrations()`
/// function collecting each module's `register()` return value.
///
/// Convention: each module must export
///   `pub fn register() -> {registration_type}`
///
/// This is the Rust equivalent of Python's __init__.py auto-discovery:
/// drop a new .rs file in the directory, it gets picked up automatically.
fn generate_registry(dir: &Path, registration_type: &str) {
    let mut modules = Vec::new();

    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "rs") {
                let stem = path.file_stem().unwrap().to_str().unwrap().to_string();
                if stem != "mod" {
                    modules.push(stem);
                }
            }
        }
    }

    modules.sort();

    // Extract just the struct name from the full path for use in fn signature.
    // e.g. "crate::gpu::filter::FilterPipelineRegistration" -> "FilterPipelineRegistration"
    let type_name = registration_type.rsplit("::").next().unwrap();

    let mut code = String::new();
    code.push_str("// @generated by build.rs — do not edit manually.\n");
    code.push_str("// To add a new module, create a .rs file in this directory\n");
    code.push_str(&format!(
        "// that exports `pub fn register() -> {registration_type}`.\n\n"
    ));

    for m in &modules {
        code.push_str(&format!("pub mod {m};\n"));
    }

    code.push_str(&format!("\nuse {registration_type};\n\n"));
    // Skip rustfmt on the generated body — layout varies across rustfmt
    // versions (single-element `vec![]` collapses on newer versions),
    // which would otherwise make CI's fmt check depend on the toolchain.
    code.push_str("#[rustfmt::skip]\n");
    code.push_str(&format!("pub fn registrations() -> Vec<{type_name}> {{\n"));
    code.push_str("    vec![\n");
    for m in &modules {
        code.push_str(&format!("        {m}::register(),\n"));
    }
    code.push_str("    ]\n");
    code.push_str("}\n");

    let mod_path = dir.join("mod.rs");
    let existing = fs::read_to_string(&mod_path).unwrap_or_default();
    if existing != code {
        fs::write(&mod_path, code).unwrap();
    }

    println!("cargo:rerun-if-changed={}", dir.display());
}

/// Scan every `.rs` file under `engine/` for methods tagged `#[handler]` (the
/// inner marker inside a `#[handlers] impl DarklyEngine` block) and emit
/// `OUT_DIR/handler_registry_gen.rs` with a `macro_registrations()` that calls
/// each generated `DarklyEngine::__darkly_handler_<name>()`.
///
/// This is the wasm-safe stand-in for `linkme` (which doesn't compile on
/// `wasm32-unknown-unknown`): the proc-macro emits the per-method registration
/// fns, and this scan — keyed off the same method names — aggregates them, the
/// same "discover by scanning source" idiom the `register()` directories use.
fn generate_handler_registry(engine_dir: &Path) {
    let mut method_names: Vec<String> = Vec::new();
    collect_handler_methods(engine_dir, &mut method_names);
    method_names.sort();
    method_names.dedup();

    let mut code = String::new();
    code.push_str("// @generated by build.rs — do not edit manually.\n");
    code.push_str(
        "// Aggregates every `#[handler]`-tagged engine method (scanned from src/engine).\n",
    );
    code.push_str("#[rustfmt::skip]\n");
    code.push_str(
        "pub fn macro_registrations() -> Vec<crate::engine::protocol::RequestRegistration> {\n",
    );
    code.push_str("    vec![\n");
    for name in &method_names {
        code.push_str(&format!(
            "        crate::engine::DarklyEngine::__darkly_handler_{name}(),\n"
        ));
    }
    code.push_str("    ]\n");
    code.push_str("}\n");

    let out_dir = env::var("OUT_DIR").expect("OUT_DIR not set");
    let out_path = PathBuf::from(out_dir).join("handler_registry_gen.rs");
    fs::write(&out_path, code).unwrap();

    println!("cargo:rerun-if-changed={}", engine_dir.display());
}

/// Recursively walk `dir`, parsing each `.rs` file and collecting the names of
/// methods carrying a `#[handler]` attribute (in any `impl` block, including
/// ones nested in inline `mod`s).
fn collect_handler_methods(dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_handler_methods(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            // Cheap pre-filter: skip files that can't contain the marker.
            if !text.contains("#[handler") {
                continue;
            }
            if let Ok(file) = syn::parse_file(&text) {
                collect_from_items(&file.items, out);
            }
        }
    }
}

fn collect_from_items(items: &[syn::Item], out: &mut Vec<String>) {
    for item in items {
        match item {
            syn::Item::Impl(item_impl) => {
                for impl_item in &item_impl.items {
                    if let syn::ImplItem::Fn(method) = impl_item {
                        if method.attrs.iter().any(|a| a.path().is_ident("handler")) {
                            out.push(method.sig.ident.to_string());
                        }
                    }
                }
            }
            syn::Item::Mod(item_mod) => {
                if let Some((_, nested)) = &item_mod.content {
                    collect_from_items(nested, out);
                }
            }
            _ => {}
        }
    }
}

/// Like [`generate_registry`], but each module exports
///   `pub fn registrations() -> Vec<{registration_type}>`
/// (a *group* of registrations) rather than a single `register()`. The
/// generated `mod.rs` flattens every module's group into one aggregate
/// `registrations()`. Used by the request protocol, where related request
/// kinds are grouped per domain file (e.g. `layers.rs`, `selection.rs`)
/// instead of one file per kind.
fn generate_grouped_registry(dir: &Path, registration_type: &str) {
    let mut modules = Vec::new();

    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "rs") {
                let stem = path.file_stem().unwrap().to_str().unwrap().to_string();
                if stem != "mod" {
                    modules.push(stem);
                }
            }
        }
    }

    modules.sort();

    let type_name = registration_type.rsplit("::").next().unwrap();

    let mut code = String::new();
    code.push_str("// @generated by build.rs — do not edit manually.\n");
    code.push_str("// To add request kinds, create or edit a domain .rs file in this\n");
    code.push_str(&format!(
        "// directory that exports `pub fn registrations() -> Vec<{registration_type}>`.\n\n"
    ));

    for m in &modules {
        code.push_str(&format!("pub mod {m};\n"));
    }

    code.push_str(&format!("\nuse {registration_type};\n\n"));
    code.push_str("#[rustfmt::skip]\n");
    code.push_str(&format!("pub fn registrations() -> Vec<{type_name}> {{\n"));
    code.push_str("    let mut all = Vec::new();\n");
    for m in &modules {
        code.push_str(&format!("    all.extend({m}::registrations());\n"));
    }
    code.push_str("    all\n");
    code.push_str("}\n");

    let mod_path = dir.join("mod.rs");
    let existing = fs::read_to_string(&mod_path).unwrap_or_default();
    if existing != code {
        fs::write(&mod_path, code).unwrap();
    }

    println!("cargo:rerun-if-changed={}", dir.display());
}

/// Scan `presets/*.yaml` and emit a generated Rust module to `OUT_DIR` with
/// one `include_str!` per YAML file plus a `defaults()` constant and an
/// `overlays()` function returning the editor-flavored overlays in
/// alphabetical order. `defaults.yaml` is required; the build panics if
/// it's missing. Every other `.yaml` becomes an equal-status overlay whose
/// display name is the file stem (Title Case).
fn generate_yaml_presets(dir: &Path) {
    let mut defaults_path: Option<PathBuf> = None;
    let mut overlays: Vec<(String, PathBuf)> = Vec::new();

    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.extension().is_some_and(|e| e == "yaml" || e == "yml") {
                continue;
            }
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            if stem == "defaults" {
                defaults_path = Some(path);
            } else if !stem.is_empty() {
                overlays.push((stem, path));
            }
        }
    }

    let defaults_path =
        defaults_path.unwrap_or_else(|| panic!("presets/defaults.yaml is required"));

    // Display-name comes from the YAML's `name:` field; fall back to a
    // titlecased file stem if the YAML doesn't set one. Order alphabetically
    // (by stem) so no editor is privileged.
    overlays.sort_by(|a, b| a.0.cmp(&b.0));

    let mut display_names: Vec<(String, String)> = Vec::new();
    for (stem, path) in &overlays {
        let yaml = fs::read_to_string(path).unwrap_or_default();
        let name = parse_yaml_display_name(&yaml).unwrap_or_else(|| titlecase(stem));
        display_names.push((stem.clone(), name));
    }

    let mut code = String::new();
    code.push_str("// @generated by build.rs — do not edit manually.\n");
    code.push_str(
        "// To add a new editor overlay, drop `<name>.yaml` in `crates/darkly/presets/`.\n\n",
    );

    code.push_str(&format!(
        "pub const DEFAULTS_YAML: &str = include_str!({:?});\n\n",
        defaults_path.display().to_string()
    ));

    for (stem, path) in &overlays {
        code.push_str(&format!(
            "const {}_YAML: &str = include_str!({:?});\n",
            stem.to_uppercase().replace('-', "_"),
            path.display().to_string()
        ));
    }
    code.push('\n');

    // Equal-status overlay list: (display_name, yaml_source).
    code.push_str("pub const OVERLAYS: &[(&str, &str)] = &[\n");
    for (stem, name) in &display_names {
        code.push_str(&format!(
            "    ({:?}, {}_YAML),\n",
            name,
            stem.to_uppercase().replace('-', "_")
        ));
    }
    code.push_str("];\n\n");

    // BASE_SETTINGS_OPTIONS feeds the `app.baseSettings` enum schema.
    code.push_str("pub const BASE_SETTINGS_OPTIONS: &[(&str, &str)] = &[\n");
    for (_, name) in &display_names {
        code.push_str(&format!("    ({:?}, {:?}),\n", name, name));
    }
    code.push_str("];\n");

    let out_dir = env::var("OUT_DIR").expect("OUT_DIR not set");
    let out_path = PathBuf::from(out_dir).join("presets_gen.rs");
    fs::write(&out_path, code).unwrap();

    println!("cargo:rerun-if-changed={}", dir.display());
}

/// Pull the `name:` field out of a YAML preset file. Lightweight — we don't
/// want a full YAML parser in build.rs, and the field's expected to be a
/// top-level scalar on its own line.
fn parse_yaml_display_name(yaml: &str) -> Option<String> {
    for line in yaml.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("name:") {
            let v = rest.trim();
            // Strip quotes if the value is quoted.
            let v = v.trim_matches('"').trim_matches('\'');
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

fn titlecase(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Scan `brushes/*.yaml` and emit a generated module to `OUT_DIR` with
/// one `include_str!` per YAML file plus a `BUILTIN_BRUSHES_YAML`
/// constant listing each `(filename, yaml_source)` pair. Built-in
/// brushes are loaded by `crate::brush::builtin_brushes::all()` at
/// engine startup — adding a new one is "drop a `.yaml` file in the
/// directory" with no other code touched.
///
/// Also records the directory as a catalog source, from the scan itself
/// rather than from a second hand-written name — the same "derived from
/// what is on disk" property [`generate_catalog_registry`] gives the
/// module directories, and what `every_catalog_source_is_exported` rests on.
fn generate_builtin_brushes(dir: &Path, catalog_sources: &mut Vec<(String, String)>) {
    record_catalog_source(
        dir.file_name()
            .and_then(|s| s.to_str())
            .expect("brush directory has a name"),
        "crate::brush::builtin_brushes",
        catalog_sources,
    );

    let mut brushes: Vec<(String, PathBuf)> = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.extension().is_some_and(|e| e == "yaml" || e == "yml") {
                continue;
            }
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            if stem.is_empty() {
                continue;
            }
            brushes.push((stem, path));
        }
    }
    brushes.sort_by(|a, b| a.0.cmp(&b.0));

    let mut code = String::new();
    code.push_str("// @generated by build.rs — do not edit manually.\n");
    code.push_str("// To add a new built-in brush, drop `<name>.yaml` in\n");
    code.push_str("// `crates/darkly/brushes/`. It is loaded automatically.\n\n");

    for (stem, path) in &brushes {
        code.push_str(&format!(
            "const {}_YAML: &str = include_str!({:?});\n",
            stem.to_uppercase().replace('-', "_"),
            path.display().to_string()
        ));
    }
    code.push('\n');

    code.push_str("pub const BUILTIN_BRUSHES_YAML: &[(&str, &str)] = &[\n");
    for (stem, _) in &brushes {
        let filename = format!("{stem}.yaml");
        code.push_str(&format!(
            "    ({:?}, {}_YAML),\n",
            filename,
            stem.to_uppercase().replace('-', "_"),
        ));
    }
    code.push_str("];\n");

    let out_dir = env::var("OUT_DIR").expect("OUT_DIR not set");
    let out_path = PathBuf::from(out_dir).join("builtin_brushes_gen.rs");
    fs::write(&out_path, code).unwrap();

    println!("cargo:rerun-if-changed={}", dir.display());
}

/// Scan `resources/textures/*.{jpg,jpeg,png,webp}` and emit a generated
/// Rust constant to `OUT_DIR` so [`crate::gpu::texture_registry`] can
/// register each image at engine init by its file basename (sans
/// extension). Mirrors the "drop a file in, it shows up" pattern that
/// `generate_registry` provides for code modules and that
/// `generate_yaml_presets` provides for editor overlays.
///
/// Dotfiles and non-image extensions are skipped — Krita autosaves
/// (`.foo.png-autosave.kra`) won't get registered as textures.
fn generate_texture_registry(dir: &Path) {
    let mut textures: Vec<(String, PathBuf)> = Vec::new();

    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            // Skip dotfiles (`.foo.png-autosave.kra`, etc).
            if stem.is_empty() || stem.starts_with('.') {
                continue;
            }
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase())
                .unwrap_or_default();
            if !matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "webp") {
                continue;
            }
            textures.push((stem, path));
        }
    }

    // Stable order so the generated file is deterministic across builds.
    textures.sort_by(|a, b| a.0.cmp(&b.0));

    let mut code = String::new();
    code.push_str("// @generated by build.rs — do not edit manually.\n");
    code.push_str("// To add a new built-in texture, drop an image into\n");
    code.push_str("// `crates/darkly/resources/textures/`. It is registered\n");
    code.push_str("// under its file basename (sans extension).\n\n");
    // Emit paths relative to `CARGO_MANIFEST_DIR` so the generated
    // file is portable across checkouts — no local absolute paths
    // baked in. `include_bytes!` resolves `concat!(env!(...), "...")`
    // at compile time against whatever machine is building.
    code.push_str("pub const BUILTIN_TEXTURES: &[(&str, &[u8])] = &[\n");
    for (stem, path) in &textures {
        let file_name = path
            .file_name()
            .and_then(|s| s.to_str())
            .expect("texture path must have a file name");
        let rel = format!("/resources/textures/{file_name}");
        code.push_str(&format!(
            "    ({stem:?}, include_bytes!(concat!(env!(\"CARGO_MANIFEST_DIR\"), {rel:?}))),\n"
        ));
    }
    code.push_str("];\n");

    let out_dir = env::var("OUT_DIR").expect("OUT_DIR not set");
    let out_path = PathBuf::from(out_dir).join("textures_gen.rs");
    fs::write(&out_path, code).unwrap();

    println!("cargo:rerun-if-changed={}", dir.display());
}
