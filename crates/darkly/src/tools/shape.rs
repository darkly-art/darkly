//! One tool per shape family. A family's registration carries its tool
//! identity (glyph, hotkey action); this module projects it into the tools
//! catalog, so a new family is a new tool with no edit here.

use crate::tool::ToolRegistration;

pub fn registrations() -> Vec<ToolRegistration> {
    crate::shape::registry()
        .iter()
        .map(|f| ToolRegistration {
            type_id: f.type_id,
            display_name: f.display_name,
            icon: f.icon,
            description: f.description,
            hotkey_action: f.hotkey_action,
            // The family schema is the `shapes` catalog's, edited in the
            // properties panel after placement.
            params: &[],
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_family_is_a_tool() {
        let tools = crate::tool::registry();
        for f in crate::shape::registry() {
            let t = tools
                .types()
                .into_iter()
                .find(|t| t.type_id == f.type_id)
                .unwrap_or_else(|| panic!("no tool for family `{}`", f.type_id));
            assert_eq!(t.display_name, f.display_name);
            assert_eq!(t.description, f.description);
            assert_eq!(t.icon, f.icon);
            assert_eq!(t.hotkey_action, f.hotkey_action);
        }
        assert!(tools.types().iter().all(|t| t.type_id != "shape"));
    }
}
