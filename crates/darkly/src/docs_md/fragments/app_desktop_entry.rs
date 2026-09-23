//! `# darkly:app-desktop-entry`: the fields of a desktop entry that restate
//! product metadata.
//!
//! `Comment`, `Categories` and `Keywords` are one region rather than three
//! because they are one fact from one source and a desktop entry has no
//! structure to interleave them with. Everything else in the file (`Exec`,
//! `Icon`, `Type`) is about how the app is launched, not what it is, and stays
//! hand-written outside the region.

use crate::docs_md::{FragmentCtx, FragmentError, FragmentRegistration};
use crate::product::product;

pub fn register() -> FragmentRegistration {
    FragmentRegistration {
        id: "app-desktop-entry",
        args: &[],
        render,
    }
}

/// Desktop-entry list values are semicolon separated and semicolon terminated.
fn list(values: &[String]) -> String {
    values
        .iter()
        .map(|v| format!("{v};"))
        .collect::<Vec<_>>()
        .join("")
}

fn render(_ctx: &FragmentCtx) -> Result<String, FragmentError> {
    let product = product();
    Ok(format!(
        "Comment={}\nCategories={}\nKeywords={}\n",
        product.summary,
        list(&product.categories),
        list(&product.keywords),
    ))
}
