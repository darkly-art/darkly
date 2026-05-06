//! Architectural invariant: `LayerRect` is a function-local translation
//! type, never long-lived storage.
//!
//! Long-lived rect storage must use `CanvasRect`. `LayerRect` is permitted
//! only as a function parameter, return type, or local `let` binding —
//! never as a struct field or enum variant field, except in the type
//! definition itself and the canvas <-> layer translation helpers.

use std::fs;
use std::path::Path;

/// Files that legitimately mention `LayerRect` in stored positions:
/// the type definition and the conversion helpers. Everything else
/// must use `CanvasRect` for storage.
const ALLOWED_FILES: &[&str] = &[
    "src/coord.rs",     // type definition + LayerRect impl
    "src/gpu/atlas.rs", // canvas <-> layer translation helpers
];

#[test]
fn layer_rect_is_never_stored() {
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src_root = crate_root.join("src");
    let mut violations = Vec::new();
    scan_dir(&src_root, crate_root, &mut violations);
    assert!(
        violations.is_empty(),
        "LayerRect appears as a struct field outside whitelisted modules:\n  {}\n\n\
         The Storage Frame Rule: long-lived rect storage must use CanvasRect.\n\
         LayerRect is a function-local translation type used only at the\n\
         wgpu boundary.",
        violations.join("\n  "),
    );
}

#[test]
fn whitelist_paths_resolve() {
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for rel in ALLOWED_FILES {
        let path = crate_root.join(rel);
        assert!(
            path.exists(),
            "whitelisted file does not exist: {} (a rename or move silently \
             dropped it out of enforcement scope)",
            rel,
        );
    }
}

#[test]
fn detector_catches_synthetic_violation() {
    // Defends against a refactor of the visitor that accidentally stops
    // detecting the pattern it exists to catch.
    let src = "struct Foo { rect: LayerRect }";
    let file = syn::parse_file(src).expect("synthetic source must parse");
    let mut visitor = FieldVisitor { violations: vec![] };
    syn::visit::visit_file(&mut visitor, &file);
    assert_eq!(
        visitor.violations.len(),
        1,
        "detector failed to flag a `LayerRect` struct field; got {:?}",
        visitor.violations,
    );
    assert_eq!(visitor.violations[0].0, "Foo");
    assert_eq!(visitor.violations[0].1, "rect");
}

fn scan_dir(dir: &Path, crate_root: &Path, out: &mut Vec<String>) {
    for entry in fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_dir(&path, crate_root, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            scan_file(&path, crate_root, out);
        }
    }
}

fn scan_file(path: &Path, crate_root: &Path, out: &mut Vec<String>) {
    let rel = path.strip_prefix(crate_root).unwrap();
    let rel_str = rel.to_string_lossy().replace('\\', "/");
    if ALLOWED_FILES.iter().any(|f| rel_str == *f) {
        return;
    }
    let src = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return,
    };
    let file = match syn::parse_file(&src) {
        Ok(f) => f,
        Err(_) => return,
    };
    let mut visitor = FieldVisitor { violations: vec![] };
    syn::visit::visit_file(&mut visitor, &file);
    for (owner, field_name) in visitor.violations {
        out.push(format!("{}: {}.{}", rel_str, owner, field_name));
    }
}

struct FieldVisitor {
    violations: Vec<(String, String)>,
}

impl<'ast> syn::visit::Visit<'ast> for FieldVisitor {
    fn visit_item_struct(&mut self, s: &'ast syn::ItemStruct) {
        for field in &s.fields {
            if type_mentions_layer_rect(&field.ty) {
                let field_name = field
                    .ident
                    .as_ref()
                    .map(|i| i.to_string())
                    .unwrap_or_else(|| "<tuple>".into());
                self.violations.push((s.ident.to_string(), field_name));
            }
        }
        syn::visit::visit_item_struct(self, s);
    }

    fn visit_variant(&mut self, v: &'ast syn::Variant) {
        for field in &v.fields {
            if type_mentions_layer_rect(&field.ty) {
                let field_name = field
                    .ident
                    .as_ref()
                    .map(|i| i.to_string())
                    .unwrap_or_else(|| "<tuple>".into());
                self.violations
                    .push((format!("variant {}", v.ident), field_name));
            }
        }
        syn::visit::visit_variant(self, v);
    }
}

fn type_mentions_layer_rect(ty: &syn::Type) -> bool {
    let mut found = false;
    struct PathVisitor<'a> {
        found: &'a mut bool,
    }
    impl<'ast, 'a> syn::visit::Visit<'ast> for PathVisitor<'a> {
        fn visit_path(&mut self, p: &'ast syn::Path) {
            if p.segments.iter().any(|s| s.ident == "LayerRect") {
                *self.found = true;
            }
            syn::visit::visit_path(self, p);
        }
    }
    syn::visit::visit_type(&mut PathVisitor { found: &mut found }, ty);
    found
}
