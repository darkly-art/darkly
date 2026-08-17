//! `<!-- darkly:catalog-table catalog=<id> -->` — one markdown row per entry of
//! a catalog, with the entry's rendered still where it has one.
//!
//! Not veil-specific: `catalog` names anything [`crate::catalog::catalogs`]
//! produces, so the same fragment documents voids, filters, blend modes or
//! brushes the day a page wants one.
//!
//! Every name and description here is a `&'static str` in the registration that
//! owns it. Fixing a typo visible in a generated table means editing
//! `crates/darkly/src/**` and re-running the sync — editing the markdown only
//! survives until the next run.

use crate::catalog::catalogs;
use crate::docs_md::{FragmentCtx, FragmentError, FragmentRegistration, STILLS_DIR};

/// Rendered width of a still in the table, in CSS pixels.
///
/// The assets are [`PREVIEW_MAX_DIM`](crate::gpu::preview::PREVIEW_MAX_DIM)
/// squares — 256 — which is the ceiling worth asking for: past it a browser is
/// upscaling what the renderer wrote.
const STILL_WIDTH: u32 = 200;

pub fn register() -> FragmentRegistration {
    FragmentRegistration {
        id: "catalog-table",
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

    let mut out = String::from("|  | Name | What it does |\n| :-: | --- | --- |\n");
    for entry in &catalog.entries {
        // An entry with no preview leaves the cell empty rather than the row
        // out — it is still part of the catalog.
        let still = if entry.supports_preview {
            let path = ctx.link(&format!(
                "{STILLS_DIR}/{}/{}.jpg",
                catalog.id, entry.type_id
            ));
            format!(
                "<img src=\"{path}\" width=\"{STILL_WIDTH}\" alt=\"{}\">",
                entry.display_name
            )
        } else {
            String::new()
        };
        out.push_str(&format!(
            "| {still} | **{}** | {} |\n",
            entry.display_name,
            cell(entry.description.unwrap_or_default()),
        ));
    }
    Ok(out)
}

/// A registration's prose as a table cell. Descriptions are written for a
/// tooltip, so nothing stops one containing the character that ends a column.
fn cell(text: &str) -> String {
    text.replace('|', r"\|")
}
