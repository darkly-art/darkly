//! Every command the editor can run, as data.
//!
//! An action has two halves. The documentable half (its id, label, icon and
//! one-line description) is static data, and lives here beside the
//! `presets/*.yaml` bindings that name the same ids. The behavioural half
//! (what running it does) closes over Svelte runes and lives in
//! `frontend/src/actions/`. The two join by id.
//!
//! Actions group one file per category (`actions/edit.rs`, `actions/view.rs`,
//! …), each a `const ACTIONS` table plus one `register()`: the same
//! many-items-per-file shape `config/sections/` uses, and the shape GIMP's
//! per-domain `GimpActionEntry` tables use. `build.rs` discovers the files, so
//! a new category is a new file and nothing else.

use crate::catalog::{Catalog, CatalogEntry};

/// One action's documentation.
pub struct ActionDef {
    /// Stable id, named by the bindings in `presets/*.yaml` and by the handler
    /// that implements the action.
    pub id: &'static str,
    pub display_name: &'static str,
    /// One sentence describing what running the action does. The command
    /// palette's substring search indexes it, so it should carry the words an
    /// artist would reach for.
    pub description: &'static str,
    /// Iconify name, rendered in the menu gutter, the command-palette row and
    /// the reference manual's table.
    pub icon: &'static str,
}

/// What each file in `actions/` returns from its `register()`: a category's id
/// and every action in it. A category is a group rather than an item, so one
/// file carries many actions and states the grouping once.
pub struct ActionCategory {
    /// Grouping id, also the label the cheat sheet and the hotkeys tab show.
    pub id: &'static str,
    pub actions: &'static [ActionDef],
}

/// Id of the catalog this registry projects into.
pub const CATALOG_ID: &str = "actions";

impl ActionDef {
    pub fn catalog_entry(&self, category: &'static str) -> CatalogEntry {
        CatalogEntry::new(self.id, self.display_name)
            .with_icon(self.icon)
            .with_description(self.description)
            .with_category(category)
            // An action *is* the thing a binding names, so the id it binds is
            // its own. Declaring it means one rule ("the entry whose
            // `hotkey_action` matches") resolves a bound chord to its
            // documentation for tools, filters and actions alike.
            .with_hotkey_action(self.id)
    }
}

/// The action catalog: every registered action, grouped by category.
pub fn catalog() -> Catalog {
    let categories = crate::actions::registrations();
    Catalog::new(
        CATALOG_ID,
        "Actions",
        categories
            .iter()
            .flat_map(|cat| cat.actions.iter().map(|a| a.catalog_entry(cat.id)))
            .collect(),
    )
    .with_description(
        "Every command the editor can run, whether from a menu, the command palette, or a hotkey.",
    )
    .with_shared_icons()
}

#[cfg(test)]
mod tests {
    /// The category is the grouping the cheat sheet and the hotkeys tab render,
    /// and it is declared once per file, so two files claiming the same one
    /// would silently merge into a section with no single owner.
    #[test]
    fn every_category_declares_a_unique_id() {
        let categories = crate::actions::registrations();
        let mut seen: Vec<&str> = Vec::new();
        for cat in &categories {
            assert!(!cat.id.is_empty(), "an action category has an empty id");
            assert!(
                !cat.actions.is_empty(),
                "action category `{}` declares no actions",
                cat.id
            );
            assert!(
                !seen.contains(&cat.id),
                "two action categories claim the id `{}`",
                cat.id
            );
            seen.push(cat.id);
        }
    }

    /// Every action carries the four columns the reference manual's table has.
    /// `catalog.rs` demands the name and the description of every catalog
    /// entry, but not the icon: that is `Option` there because other
    /// registries decline it, whereas an action always has one to show in the
    /// menu gutter.
    #[test]
    fn every_action_declares_an_id_and_an_icon() {
        for cat in crate::actions::registrations() {
            for a in cat.actions {
                assert!(
                    !a.id.is_empty(),
                    "an action in `{}` has an empty id",
                    cat.id
                );
                assert!(!a.icon.is_empty(), "action `{}` declares no icon", a.id);
            }
        }
    }
}
