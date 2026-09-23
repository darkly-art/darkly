//! `<!-- darkly:app-description -->`: the long description, as AppStream's
//! `<description>` of `<p>` elements.
//!
//! One source (the `description` of [`crate::product`]) behind however many
//! syntaxes want it. A second target is a `format` argument and a second
//! renderer here, not a second copy of the text.

use crate::docs_md::{FragmentCtx, FragmentError, FragmentRegistration};
use crate::product::{self, product};

pub fn register() -> FragmentRegistration {
    FragmentRegistration {
        id: "app-description",
        args: &[],
        render,
    }
}

fn render(_ctx: &FragmentCtx) -> Result<String, FragmentError> {
    Ok(format!(
        "  <description>\n    <p>{}</p>\n  </description>\n",
        product::xml_escape(&product().description)
    ))
}
