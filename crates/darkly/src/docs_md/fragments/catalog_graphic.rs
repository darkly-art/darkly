//! `<!-- darkly:catalog-graphic catalog=<id> [category=<name>] -->`: one
//! rendered picture of a catalog, in place of a table of it.
//!
//! `category` narrows the picture to one of the catalog's grouping labels, for
//! a catalog whose entries are several kinds of thing and whose prose covers
//! them a group at a time. Adding an entry to a group the region does not name
//! leaves that region alone, which is the point: the picture stays honest about
//! exactly what it depicts.
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

use crate::catalog::{catalogs, CatalogEntry};
use crate::docs_md::{FragmentCtx, FragmentError, FragmentRegistration};

/// Where rendered catalog graphics live, relative to the repository root.
///
/// CANONICAL TWIN of `GRAPHICS_DIR` in
/// `frontend/scripts/render-doc-graphics.mjs`, which writes what this links to.
/// Unlike [`STILLS_DIR`](crate::docs_md::STILLS_DIR), whose two consumers are
/// both Rust, the other half of this pair is a node script and cannot import a
/// Rust const under any arrangement. If you move one, move the other.
const GRAPHICS_DIR: &str = "docs/images/graphics";

/// The file stem of a catalog graphic: the catalog id, plus the category when a
/// region names one, so one catalog can carry a picture per category without
/// two of them claiming the same file.
///
/// CANONICAL TWIN of `graphicName` in
/// `frontend/scripts/render-doc-graphics.mjs`, which writes the file this
/// names. Same reason as [`GRAPHICS_DIR`]: the other half is a node script and
/// cannot import a Rust function. If you change one, change the other.
pub fn graphic_name(catalog: &str, category: Option<&str>) -> String {
    match category {
        Some(c) => format!("{catalog}-{}", slug(c)),
        None => catalog.to_string(),
    }
}

/// A display label as a filename component: lowercase, non-alphanumerics to
/// hyphens ("Black and White" -> "black-and-white").
fn slug(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

pub fn register() -> FragmentRegistration {
    FragmentRegistration {
        id: "catalog-graphic",
        args: &["catalog", "category"],
        render,
    }
}

fn render(ctx: &FragmentCtx) -> Result<String, FragmentError> {
    let id = ctx.arg("catalog")?;
    let catalog = catalogs()
        .into_iter()
        .find(|c| c.id == id)
        .ok_or_else(|| FragmentError::new(format!("no catalog named `{id}`")))?;

    let category = ctx.arg_opt("category");
    let entries: Vec<&CatalogEntry> = match category {
        Some(want) => catalog
            .entries
            .iter()
            .filter(|e| e.category == Some(want))
            .collect(),
        None => catalog.entries.iter().collect(),
    };
    // A graphic has no error state, so an empty selection is refused here
    // rather than linking a picture of nothing.
    if entries.is_empty() {
        return Err(FragmentError::new(match category {
            Some(want) => format!("catalog `{id}` has no entries in category `{want}`"),
            None => format!("catalog `{id}` is empty"),
        }));
    }

    // The category names the picture when there is one: it is what the entries
    // have in common, and what the surrounding prose calls them.
    let title = category.unwrap_or(catalog.title);
    let names: Vec<&str> = entries.iter().map(|e| e.display_name).collect();
    let alt = format!("{title}: {}", names.join(", "));
    let src = ctx.link(&format!(
        "{GRAPHICS_DIR}/{}.jpg",
        graphic_name(catalog.id, category)
    ));

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

    fn render_category(catalog: &str, category: &str) -> Result<String, FragmentError> {
        let ctx = FragmentCtx {
            args: BTreeMap::from([("catalog", catalog), ("category", category)]),
            md_dir: Path::new(""),
        };
        render(&ctx)
    }

    #[test]
    fn names_every_entry_of_the_catalog() {
        let out = render_for("effects", "").expect("effects renders");
        let effects = catalogs().into_iter().find(|c| c.id == "effects").unwrap();
        assert!(!effects.entries.is_empty(), "no effects to assert about");
        for entry in &effects.entries {
            assert!(
                out.contains(entry.display_name),
                "`{}` missing from alt text: {out}",
                entry.display_name
            );
        }
        assert!(out.contains(effects.title), "catalog title missing: {out}");
    }

    #[test]
    fn links_the_rendered_graphic() {
        let out = render_for("effects", "").expect("effects renders");
        assert!(
            out.contains("src=\"docs/images/graphics/effects.jpg\""),
            "unexpected src: {out}"
        );
    }

    /// Markdown resolves relative links against the file, so a region in a
    /// nested page has to reach back out. Same contract `catalog_table` relies
    /// on for its stills.
    #[test]
    fn the_link_is_relative_to_the_markdown_file() {
        let out = render_for("effects", "docs/manual").expect("effects renders");
        assert!(
            out.contains("src=\"../images/graphics/effects.jpg\""),
            "unexpected src: {out}"
        );
    }

    /// The narrowed picture names its category, lists only that category's
    /// entries, and links a file of its own, so two categories of one catalog
    /// cannot overwrite each other's image.
    #[test]
    fn a_category_narrows_the_picture_to_that_group() {
        let out = render_category("effects", "Veils").expect("veils renders");
        let effects = catalogs().into_iter().find(|c| c.id == "effects").unwrap();
        let (veils, others): (Vec<_>, Vec<_>) = effects
            .entries
            .iter()
            .partition(|e| e.category == Some("Veils"));
        assert!(!veils.is_empty() && !others.is_empty(), "need both groups");

        for entry in &veils {
            assert!(
                out.contains(entry.display_name),
                "`{}` missing from alt text: {out}",
                entry.display_name
            );
        }
        for entry in &others {
            assert!(
                !out.contains(entry.display_name),
                "`{}` is not a veil and must not be listed: {out}",
                entry.display_name
            );
        }
        assert!(
            out.contains("alt=\"Veils:"),
            "category title missing: {out}"
        );
        assert!(
            out.contains("src=\"docs/images/graphics/effects-veils.jpg\""),
            "unexpected src: {out}"
        );
    }

    #[test]
    fn a_category_no_entry_declares_is_an_error() {
        let err = render_category("effects", "Nope").expect_err("empty group must fail");
        assert!(
            err.0.contains("Nope"),
            "error should name the category: {}",
            err.0
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
