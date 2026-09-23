//! `<!-- darkly:app-summary -->`: the AppStream `<summary>` element.
//!
//! The text is the `summary` of [`crate::product`]. Editing it here lasts until
//! the next sync.

use crate::docs_md::{FragmentCtx, FragmentError, FragmentRegistration};
use crate::product;

pub fn register() -> FragmentRegistration {
    FragmentRegistration {
        id: "app-summary",
        args: &[],
        render,
    }
}

fn render(_ctx: &FragmentCtx) -> Result<String, FragmentError> {
    Ok(format!(
        "  <summary>{}</summary>\n",
        product::xml_escape(&product::product().summary)
    ))
}
