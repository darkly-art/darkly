//! The generated regions of this repository's own markdown, checked against the
//! registries they are generated from.
//!
//! `docs_md`'s unit tests cover the machinery — the marker grammar, relative
//! links, idempotence. What is left is the part that can only be asserted
//! against the checkout: that what is committed matches what the registries say
//! today, and that everything those regions point a reader at exists.
//!
//! Needs no GPU. That is the whole reason the text half and the image half are
//! separate commands: this runs in the ordinary suite, on every change to any
//! registration, which is exactly when a README drifts.
//!
//! Run with: `cargo test -p darkly --test docs_md`

use std::path::{Path, PathBuf};

use darkly::docs_md::{self, Mode};

/// Committed markdown says what the registries say.
///
/// This is the test that fires when someone adds a veil, renames one, or edits
/// a `description:` — none of which look like documentation changes from inside
/// `crates/darkly/src/`.
#[test]
fn generated_regions_are_up_to_date() {
    let root = docs_md::repo_root();
    let report = docs_md::sync(&root, Mode::Check).expect("the markdown parses");

    assert!(
        !report.generated.is_empty(),
        "no generated regions found under {} — the walk is not reaching the \
         checkout, so this test is asserting nothing",
        root.display()
    );
    assert!(
        report.changed.is_empty(),
        "out of date: {}\nrun `cargo run -p darkly --bin sync-docs`",
        report
            .changed
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );
}

/// Every local image in a file carrying a generated region is on disk.
///
/// The one coupling between the two halves. Preview stills are rendered by hand
/// (`render_docs --stills`) because they need a GPU and land in the repository
/// as binaries; nothing but this stops a new veil from shipping a row with a
/// broken image in it.
///
/// Whole files rather than just the region bodies: a generated row and the
/// hand-written image above it break the same way, and there is nothing to gain
/// from checking only the half a program wrote.
#[test]
fn generated_regions_link_to_images_that_exist() {
    let root = docs_md::repo_root();
    let report = docs_md::sync(&root, Mode::Check).expect("the markdown parses");

    let mut checked = 0;
    for rel in &report.generated {
        let text = std::fs::read_to_string(root.join(rel)).expect("a file the walk just read");
        let dir = rel.parent().unwrap_or(Path::new(""));
        for src in image_sources(&text) {
            // Remote images are somebody else's to serve; only what this
            // repository is supposed to contain is checkable here.
            if src.starts_with("http") {
                continue;
            }
            let path = normalize(&root.join(dir).join(&src));
            assert!(
                path.exists(),
                "{} links to `{src}`, which is not in the checkout — run \
                 `cargo run --release -p darkly --features testing --bin render_docs \
                 -- --stills --catalog <id>`",
                rel.display()
            );
            checked += 1;
        }
    }
    assert!(
        checked > 0,
        "no local images in any generated region — nothing was asserted"
    );
}

/// Every `src="…"` in `text`. Deliberately naive: generated tables emit plain
/// `<img>` tags, and a markdown file is not worth a parser.
fn image_sources(text: &str) -> Vec<String> {
    text.split("src=\"")
        .skip(1)
        .filter_map(|rest| rest.split_once('"').map(|(src, _)| src.to_string()))
        .collect()
}

/// Resolve `..` segments textually. `Path::canonicalize` would do it, but only
/// for a path that already exists — and a missing image is what this is for.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}
