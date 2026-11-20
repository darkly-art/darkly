//! `<!-- darkly:catalog-graphic catalog=<id> [category=<name> | entry=<type_id>] -->`:
//! one rendered picture of a catalog, in place of a table of it.
//!
//! `category` narrows the picture to one of the catalog's grouping labels, for
//! a catalog whose entries are several kinds of thing and whose prose covers
//! them a group at a time. Adding an entry to a group the region does not name
//! leaves that region alone, which is the point: the picture stays honest about
//! exactly what it depicts.
//!
//! `entry` narrows it to one entry, for a catalog whose entries are each a
//! picture's worth: a brush pack, whose card shows the brushes it groups. The
//! two are exclusive, since a picture is of a group or of an entry, not both.
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
//! what the picture shows (the group's entries, or an entry's members), so
//! adding or renaming one changes the region body and the ordinary test suite
//! reports the drift, exactly as it does for a table. An image alone would take
//! the names out of the README's text entirely, where neither a search nor a
//! screen reader would find them.

use crate::catalog::{catalogs, Catalog, CatalogEntry};
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
/// region names one, or the entry when a region names one, so one catalog can
/// carry a picture per category or per entry without two of them claiming the
/// same file.
///
/// A category is a display label and is slugged; an entry is a type id, already
/// a filename component (`docs/images/previews/<catalog>/<type>.jpg`), and is
/// appended verbatim. The two never arrive together: `render` refuses that.
///
/// CANONICAL TWIN of `graphicName` in
/// `frontend/scripts/render-doc-graphics.mjs`, which writes the file this
/// names. Same reason as [`GRAPHICS_DIR`]: the other half is a node script and
/// cannot import a Rust function. If you change one, change the other.
pub fn graphic_name(catalog: &str, category: Option<&str>, entry: Option<&str>) -> String {
    match (category, entry) {
        (Some(c), _) => format!("{catalog}-{}", slug(c)),
        (None, Some(e)) => format!("{catalog}-{e}"),
        (None, None) => catalog.to_string(),
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
        args: &["catalog", "category", "entry"],
        render,
    }
}

fn render(ctx: &FragmentCtx) -> Result<String, FragmentError> {
    let id = ctx.arg("catalog")?;
    let all = catalogs();
    let catalog = all
        .iter()
        .find(|c| c.id == id)
        .ok_or_else(|| FragmentError::new(format!("no catalog named `{id}`")))?;

    let category = ctx.arg_opt("category");
    let entry = ctx.arg_opt("entry");
    if category.is_some() && entry.is_some() {
        return Err(FragmentError::new(
            "a region names either a category or an entry, not both",
        ));
    }

    // The title is what the picture is of, and the names are what it shows:
    // the group's entries, or the one entry's members.
    let (title, names): (&str, Vec<&str>) = match entry {
        Some(want) => {
            let found = catalog
                .entries
                .iter()
                .find(|e| e.type_id == want)
                .ok_or_else(|| {
                    FragmentError::new(format!("catalog `{id}` has no entry `{want}`"))
                })?;
            (found.display_name, depicted(found, &all)?)
        }
        None => {
            let entries: Vec<&CatalogEntry> = match category {
                Some(want) => catalog
                    .entries
                    .iter()
                    .filter(|e| e.category == Some(want))
                    .collect(),
                None => catalog.entries.iter().collect(),
            };
            // A graphic has no error state, so an empty selection is refused
            // here rather than linking a picture of nothing.
            if entries.is_empty() {
                return Err(FragmentError::new(match category {
                    Some(want) => format!("catalog `{id}` has no entries in category `{want}`"),
                    None => format!("catalog `{id}` is empty"),
                }));
            }
            // The category names the picture when there is one: it is what
            // the entries have in common, and what the surrounding prose
            // calls them.
            (
                category.unwrap_or(catalog.title),
                entries.iter().map(|e| e.display_name).collect(),
            )
        }
    };

    let alt = if names.is_empty() {
        title.to_string()
    } else {
        format!("{title}: {}", names.join(", "))
    };
    let src = ctx.link(&format!(
        "{GRAPHICS_DIR}/{}.jpg",
        graphic_name(catalog.id, category, entry)
    ));

    Ok(format!("<img src=\"{src}\" alt=\"{}\">\n", attr(&alt)))
}

/// What a picture of one entry shows, for its alt text: its members' display
/// names when it is a group, else its description. A member id the member
/// catalog does not have is an error, not a silent gap, for the same reason a
/// missing still is.
fn depicted<'a>(
    entry: &'a CatalogEntry,
    all: &'a [Catalog],
) -> Result<Vec<&'a str>, FragmentError> {
    let Some(members) = &entry.members else {
        return Ok(entry.description.into_iter().collect());
    };
    let of = all
        .iter()
        .find(|c| c.id == members.catalog)
        .ok_or_else(|| {
            FragmentError::new(format!(
                "`{}` groups entries of `{}`, which is not a catalog",
                entry.type_id, members.catalog
            ))
        })?;
    members
        .entries
        .iter()
        .map(|m| {
            of.entries
                .iter()
                .find(|e| e.type_id == *m)
                .map(|e| e.display_name)
                .ok_or_else(|| {
                    FragmentError::new(format!(
                        "`{}` lists `{m}`, which catalog `{}` does not have",
                        entry.type_id, members.catalog
                    ))
                })
        })
        .collect()
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

    fn render_args(args: BTreeMap<&str, &str>, md_dir: &str) -> Result<String, FragmentError> {
        let ctx = FragmentCtx {
            args,
            md_dir: Path::new(md_dir),
        };
        render(&ctx)
    }

    fn render_for(catalog: &str, md_dir: &str) -> Result<String, FragmentError> {
        render_args(BTreeMap::from([("catalog", catalog)]), md_dir)
    }

    fn render_category(catalog: &str, category: &str) -> Result<String, FragmentError> {
        render_args(
            BTreeMap::from([("catalog", catalog), ("category", category)]),
            "",
        )
    }

    fn render_entry(catalog: &str, entry: &str) -> Result<String, FragmentError> {
        render_args(BTreeMap::from([("catalog", catalog), ("entry", entry)]), "")
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

    /// A picture of one pack is titled by the pack and lists its members by
    /// display name, in declared order, and links a file of its own per pack.
    /// Built from the shipped packs and brushes rather than literals, so a
    /// pack edit cannot leave this asserting yesterday's membership.
    #[test]
    fn an_entry_narrows_the_picture_to_that_entry() {
        let packs = crate::brush::packs::docs();
        assert!(!packs.is_empty(), "no shipped packs to assert about");
        let brushes = crate::brush::builtin_brushes::docs();
        for pack in packs {
            let out = render_entry("brushPacks", &pack.id).expect("pack renders");
            let names: Vec<&str> = pack
                .members
                .iter()
                .map(|m| {
                    brushes
                        .iter()
                        .find(|(stem, _)| *stem == m.as_str())
                        .map(|(_, info)| info.name.as_str())
                        .unwrap_or_else(|| panic!("`{}` lists unknown brush `{m}`", pack.id))
                })
                .collect();
            let alt = format!("alt=\"{}: {}\"", pack.name, names.join(", "));
            assert!(out.contains(&alt), "expected `{alt}` in: {out}");
            let src = format!("src=\"docs/images/graphics/brushPacks-{}.jpg\"", pack.id);
            assert!(out.contains(&src), "expected `{src}` in: {out}");
        }
    }

    /// An entry that groups nothing is described rather than enumerated.
    #[test]
    fn an_entry_without_members_lists_its_description() {
        let effects = catalogs().into_iter().find(|c| c.id == "effects").unwrap();
        let first = &effects.entries[0];
        assert!(first.members.is_none(), "effects do not group");
        let out = render_entry("effects", first.type_id).expect("effect renders");
        let alt = format!(
            "alt=\"{}: {}\"",
            first.display_name,
            first.description.expect("effects are documented")
        );
        assert!(out.contains(&alt), "expected `{alt}` in: {out}");
        let src = format!("src=\"docs/images/graphics/effects-{}.jpg\"", first.type_id);
        assert!(out.contains(&src), "expected `{src}` in: {out}");
    }

    #[test]
    fn an_unknown_entry_is_an_error() {
        let err = render_entry("brushPacks", "nope").expect_err("unknown entry must fail");
        assert!(
            err.0.contains("nope"),
            "error should name the entry: {}",
            err.0
        );
    }

    /// A picture is of a group or of an entry; a region asking for both is a
    /// mistake to report, not a precedence to guess.
    #[test]
    fn category_and_entry_together_is_an_error() {
        let err = render_args(
            BTreeMap::from([
                ("catalog", "effects"),
                ("category", "Veils"),
                ("entry", "frozen"),
            ]),
            "",
        )
        .expect_err("both must fail");
        assert!(
            err.0.contains("category") && err.0.contains("entry"),
            "error should name both arguments: {}",
            err.0
        );
    }

    #[test]
    fn graphic_name_slugs_a_category_and_keeps_an_entry_verbatim() {
        assert_eq!(graphic_name("effects", None, None), "effects");
        assert_eq!(
            graphic_name("effects", Some("Black and White"), None),
            "effects-black-and-white"
        );
        assert_eq!(
            graphic_name("brushPacks", None, Some("dry_media")),
            "brushPacks-dry_media"
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
