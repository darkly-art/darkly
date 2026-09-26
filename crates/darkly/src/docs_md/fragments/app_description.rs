//! `<!-- darkly:app-description -->`: the long description, as AppStream's
//! `<description>` of `<p>` and `<ul>` elements, headings as `<p><em>`.
//!
//! One source (the `long_description` of [`crate::product`]) behind however
//! many syntaxes want it. A second target is a `format` argument and a second
//! renderer here, not a second copy of the text.

use crate::docs_md::{FragmentCtx, FragmentError, FragmentRegistration};
use crate::product::{self, product, Block};

pub fn register() -> FragmentRegistration {
    FragmentRegistration {
        id: "app-description",
        args: &[],
        render,
    }
}

fn render(_ctx: &FragmentCtx) -> Result<String, FragmentError> {
    let blocks: String = product()
        .long_description
        .iter()
        .map(|block| match block {
            Block::Para(p) => format!("    <p>{}</p>\n", product::xml_escape(p)),
            Block::List(items) => {
                let items: String = items
                    .iter()
                    .map(|i| format!("      <li>{}</li>\n", product::xml_escape(i)))
                    .collect();
                format!("    <ul>\n{items}    </ul>\n")
            }
            Block::Heading { heading } => {
                format!("    <p><em>{}</em></p>\n", product::xml_escape(heading))
            }
        })
        .collect();
    Ok(format!("  <description>\n{blocks}  </description>\n"))
}
