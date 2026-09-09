//! `<!-- darkly:catalog-graphic catalog=<id> -->`: one rendered picture of a
//! whole catalog, in place of a table of it.
//!
//! The sibling [`catalog_table`](super::catalog_table) fragment describes a
//! catalog; this one shows it. A table is the right shape where the prose
//! matters and a reader is looking something up; for a catalog whose entries
//! *are* images, a row of names beside thumbnails spends a column of markdown on
//! restating what the picture already says.
//!
//! The picture is authored as a Svelte component and rendered by
//! `frontend/scripts/render-doc-graphics.mjs`; nothing here draws anything. What
//! this fragment owns is the link to it and the alt text, and the alt text is
//! the reason the region is generated rather than hand-written: it enumerates
//! the catalog, so adding or renaming an entry changes the region body and the
//! ordinary test suite reports the drift, exactly as it does for a table. An
//! image alone would take the names out of the README's text entirely, where
//! neither a search nor a screen reader would find them.

use crate::catalog::catalogs;
use crate::docs_md::{FragmentCtx, FragmentError, FragmentRegistration};

/// Where rendered catalog graphics live, relative to the repository root.
///
/// CANONICAL TWIN of `GRAPHICS_DIR` in
/// `frontend/scripts/render-doc-graphics.mjs`, which writes what this links to.
/// Unlike [`STILLS_DIR`](crate::docs_md::STILLS_DIR), whose two consumers are
/// both Rust, the other half of this pair is a node script and cannot import a
/// Rust const under any arrangement. If you move one, move the other.
const GRAPHICS_DIR: &str = "docs/images/graphics";

pub fn register() -> FragmentRegistration {
    FragmentRegistration {
        id: "catalog-graphic",
        args: &["catalog"],
        render,
    }
}

fn render(ctx: &FragmentCtx) -> Result<String, FragmentError> {
    let id = ctx.arg("catalog")?;
    let catalog = catalogs()
        .into_iter()
        .find(|c| c.id == id)
        .ok_or_else(|| FragmentError::new(format!("no catalog named `{id}`")))?;

    let names: Vec<&str> = catalog.entries.iter().map(|e| e.display_name).collect();
    let alt = format!("{}: {}", catalog.title, names.join(", "));
    let src = ctx.link(&format!("{GRAPHICS_DIR}/{}.jpg", catalog.id));

    Ok(format!("<img src=\"{src}\" alt=\"{}\">\n", attr(&alt)))
}

/// Prose as an HTML attribute value. Display names are `&'static str` written
/// for a picker, so nothing stops one containing the character that ends the
/// attribute.
fn attr(text: &str) -> String {
    text.replace('&', "&amp;").replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::Path;

    fn render_for(catalog: &str, md_dir: &str) -> Result<String, FragmentError> {
        let ctx = FragmentCtx {
            args: BTreeMap::from([("catalog", catalog)]),
            md_dir: Path::new(md_dir),
        };
        render(&ctx)
    }

    #[test]
    fn names_every_entry_of_the_catalog() {
        let out = render_for("veils", "").expect("veils renders");
        let veils = catalogs().into_iter().find(|c| c.id == "veils").unwrap();
        assert!(!veils.entries.is_empty(), "no veils to assert about");
        for entry in &veils.entries {
            assert!(
                out.contains(entry.display_name),
                "`{}` missing from alt text: {out}",
                entry.display_name
            );
        }
        assert!(out.contains(veils.title), "catalog title missing: {out}");
    }

    #[test]
    fn links_the_rendered_graphic() {
        let out = render_for("veils", "").expect("veils renders");
        assert!(
            out.contains("src=\"docs/images/graphics/veils.jpg\""),
            "unexpected src: {out}"
        );
    }

    /// Markdown resolves relative links against the file, so a region in a
    /// nested page has to reach back out. Same contract `catalog_table` relies
    /// on for its stills.
    #[test]
    fn the_link_is_relative_to_the_markdown_file() {
        let out = render_for("veils", "docs/manual").expect("veils renders");
        assert!(
            out.contains("src=\"../images/graphics/veils.jpg\""),
            "unexpected src: {out}"
        );
    }

    #[test]
    fn an_unknown_catalog_is_an_error() {
        let err = render_for("nope", "").expect_err("unknown catalog must fail");
        assert!(
            err.0.contains("nope"),
            "error should name the catalog: {}",
            err.0
        );
    }
}
