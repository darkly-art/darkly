//! `<!-- darkly:app-releases -->`: the AppStream `<releases>` block, one
//! `<release>` per entry of [`crate::releases`].
//!
//! Each release links to its GitHub page and lists its changes by PR title.
//! AppStream allows no link markup inside a list item, so the PR number rides
//! as text and the per-change links live on the GitHub release page instead.
//! A release with no changes gets no `<description>`, which the validator
//! accepts.

use crate::docs_md::{FragmentCtx, FragmentError, FragmentRegistration};
use crate::product::xml_escape;
use crate::releases::{releases, Release};

pub fn register() -> FragmentRegistration {
    FragmentRegistration {
        id: "app-releases",
        args: &[],
        render,
    }
}

fn render(_ctx: &FragmentCtx) -> Result<String, FragmentError> {
    Ok(render_releases(releases()))
}

fn render_releases(releases: &[Release]) -> String {
    let mut out = String::from("  <releases>\n");
    for r in releases {
        out.push_str(&format!(
            "    <release version=\"{}\" date=\"{}\">\n      <url type=\"details\">{}</url>\n",
            xml_escape(&r.version),
            xml_escape(&r.date),
            xml_escape(&r.url())
        ));
        if !r.changes.is_empty() {
            out.push_str("      <description>\n        <ul>\n");
            for c in &r.changes {
                out.push_str(&format!(
                    "          <li>{} (#{})</li>\n",
                    xml_escape(&c.title),
                    c.pr
                ));
            }
            out.push_str("        </ul>\n      </description>\n");
        }
        out.push_str("    </release>\n");
    }
    out.push_str("  </releases>\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::releases::Change;

    #[test]
    fn renders_the_appstream_block() {
        let list = vec![
            Release {
                version: "0.8.0".into(),
                date: "2026-09-10".into(),
                changes: vec![
                    Change::new(111, "Better veils".into()).unwrap(),
                    Change::new(113, "Radial & Widget".into()).unwrap(),
                ],
            },
            Release {
                version: "0.5.0".into(),
                date: "2026-07-09".into(),
                changes: vec![],
            },
        ];
        assert_eq!(
            render_releases(&list),
            "  <releases>\n\
             \x20   <release version=\"0.8.0\" date=\"2026-09-10\">\n\
             \x20     <url type=\"details\">https://github.com/darkly-art/darkly/releases/tag/v0.8.0</url>\n\
             \x20     <description>\n\
             \x20       <ul>\n\
             \x20         <li>Better veils (#111)</li>\n\
             \x20         <li>Radial &amp; Widget (#113)</li>\n\
             \x20       </ul>\n\
             \x20     </description>\n\
             \x20   </release>\n\
             \x20   <release version=\"0.5.0\" date=\"2026-07-09\">\n\
             \x20     <url type=\"details\">https://github.com/darkly-art/darkly/releases/tag/v0.5.0</url>\n\
             \x20   </release>\n\
             \x20 </releases>\n"
        );
    }
}
