//! A brush pack — a named, iconed, two-colored group of brushes.
//!
//! A brush may belong to any number of packs: adding one to a pack copies a
//! reference, it does not move the brush. The pack is the sole authority on
//! membership; nothing on a brush records which packs hold it.

use serde::{Deserialize, Serialize};

use crate::brush::pack_icons::is_pack_icon;

/// Opaque identity for a brush. Shipped brushes use their YAML file stem;
/// a painter's brushes are given a minted id when they are saved.
///
/// Distinct from the brush *name*, which is the display value and may be
/// changed freely — that is the whole point of having an id, since a pack's
/// member list and the recent-brushes list both hold ids and so survive a
/// rename untouched.
pub type BrushId = String;

/// Opaque identity for a pack. Shipped packs use their YAML file stem.
pub type PackId = String;

/// How far a pack may be edited.
///
/// Nothing outside this module matches on this. Consumers call the `ensure_*`
/// methods, which is what keeps "Favorites is the built-in the painter may
/// fill" a fact of one line of YAML rather than a condition at a call site.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackMutability {
    /// Shipped and fixed: no brush may be added or removed, and the pack may
    /// not be renamed, restyled or deleted.
    #[default]
    Locked,
    /// Shipped, but the painter chooses what is in it. Favorites.
    Members,
    /// The painter's own. Everything about it is theirs.
    Full,
}

/// A group of brushes, as the library holds it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrushPack {
    pub id: PackId,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub icon: String,
    pub primary: String,
    pub secondary: String,
    #[serde(default)]
    pub mutability: PackMutability,
    /// The brushes in this pack, in the painter's chosen order. The sole
    /// authority on membership.
    #[serde(default)]
    pub members: Vec<BrushId>,
}

impl BrushPack {
    /// A pack the painter owns, with everything editable.
    pub fn new(
        id: impl Into<PackId>,
        name: impl Into<String>,
        icon: impl Into<String>,
        primary: impl Into<String>,
        secondary: impl Into<String>,
    ) -> Self {
        BrushPack {
            id: id.into(),
            name: name.into(),
            description: String::new(),
            icon: icon.into(),
            primary: primary.into(),
            secondary: secondary.into(),
            mutability: PackMutability::Full,
            members: Vec::new(),
        }
    }

    /// Reject an edit to this pack's member list, if it is not the painter's
    /// to make.
    pub fn ensure_members_editable(&self) -> Result<(), String> {
        match self.mutability {
            PackMutability::Locked => Err(format!(
                "brush pack '{}' is built in — its brushes cannot be changed",
                self.name
            )),
            PackMutability::Members | PackMutability::Full => Ok(()),
        }
    }

    /// Reject a change to this pack's name, description, icon, colors, or its
    /// existence.
    pub fn ensure_identity_editable(&self) -> Result<(), String> {
        match self.mutability {
            PackMutability::Locked | PackMutability::Members => Err(format!(
                "brush pack '{}' is built in and cannot be renamed, restyled or deleted",
                self.name
            )),
            PackMutability::Full => Ok(()),
        }
    }

    /// Whether the painter may add and remove brushes here.
    pub fn can_edit_members(&self) -> bool {
        self.ensure_members_editable().is_ok()
    }

    /// Whether the painter may rename, restyle or delete this pack.
    pub fn can_edit_identity(&self) -> bool {
        self.ensure_identity_editable().is_ok()
    }

    pub fn contains(&self, brush: &str) -> bool {
        self.members.iter().any(|m| m == brush)
    }

    /// Add `brush` to the end of the member list. Idempotent — a brush already
    /// present keeps its position, so re-adding it is not a reorder.
    pub fn add(&mut self, brush: BrushId) -> Result<(), String> {
        self.ensure_members_editable()?;
        if !self.contains(&brush) {
            self.members.push(brush);
        }
        Ok(())
    }

    /// Remove `brush`. Removing one that is not here is not an error: the
    /// operation is convergent, so a retry after a partial write is safe.
    pub fn remove(&mut self, brush: &str) -> Result<(), String> {
        self.ensure_members_editable()?;
        self.members.retain(|m| m != brush);
        Ok(())
    }

    /// Move `brush` to `index` within the member list.
    pub fn reorder(&mut self, brush: &str, index: usize) -> Result<(), String> {
        self.ensure_members_editable()?;
        let Some(from) = self.members.iter().position(|m| m == brush) else {
            return Err(format!(
                "brush pack '{}' does not contain that brush",
                self.name
            ));
        };
        let member = self.members.remove(from);
        let to = index.min(self.members.len());
        self.members.insert(to, member);
        Ok(())
    }

    /// Drop members that no longer name a brush that exists. Returns whether
    /// anything was dropped, so a caller can persist only when it must.
    ///
    /// Bypasses [`ensure_members_editable`] deliberately: this is not an edit
    /// the painter asked for, it is the library refusing to point at a ghost.
    pub fn retain_members(&mut self, exists: impl Fn(&str) -> bool) -> bool {
        let before = self.members.len();
        self.members.retain(|m| exists(m));
        self.members.len() != before
    }
}

/// Accept `#rrggbb` or `#rrggbbaa`, hex digits only.
///
/// Colors are validated on the way in rather than defaulted, because a pack
/// file may come from anywhere and a silently-black pack is worse than a
/// rejected one.
pub fn validate_color(value: &str, field: &str) -> Result<(), String> {
    let Some(digits) = value.strip_prefix('#') else {
        return Err(format!("{field} '{value}' must start with '#'"));
    };
    if !matches!(digits.len(), 6 | 8) {
        return Err(format!(
            "{field} '{value}' must have 6 or 8 hex digits, got {}",
            digits.len()
        ));
    }
    if !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("{field} '{value}' contains a non-hex digit"));
    }
    Ok(())
}

/// Accept a `collection:name` Iconify reference.
///
/// Shape only. Whether the icon *renders* is the frontend's question, with a
/// frontend answer: an unbundled name falls back to
/// [`PACK_ICON_FALLBACK`](crate::brush::pack_icons::PACK_ICON_FALLBACK), which
/// is what lets a third-party pack degrade gracefully instead of showing a
/// hole.
pub fn validate_icon(value: &str) -> Result<(), String> {
    match value.split_once(':') {
        Some((collection, name)) if !collection.is_empty() && !name.is_empty() => Ok(()),
        _ => Err(format!(
            "pack icon '{value}' must be a `collection:name` Iconify reference"
        )),
    }
}

/// Validate a pack the way an imported one must be: shape-checked colors and
/// icon, and a name that is actually a name.
pub fn validate_pack(name: &str, icon: &str, primary: &str, secondary: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("a brush pack needs a name".into());
    }
    validate_icon(icon)?;
    validate_color(primary, "pack primary color")?;
    validate_color(secondary, "pack secondary color")?;
    Ok(())
}

/// Validate a *shipped* pack, which is held to the stricter rule that its icon
/// must be one the renderer actually has.
pub fn validate_shipped_pack(pack: &BrushPack) -> Result<(), String> {
    validate_pack(&pack.name, &pack.icon, &pack.primary, &pack.secondary)?;
    if !is_pack_icon(&pack.icon) {
        return Err(format!(
            "shipped pack '{}' names icon '{}', which is not in PACK_ICONS and would not render",
            pack.id, pack.icon
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack(mutability: PackMutability) -> BrushPack {
        BrushPack {
            id: "p".into(),
            name: "P".into(),
            description: String::new(),
            icon: "mdi:brush".into(),
            primary: "#ffffff".into(),
            secondary: "#000000".into(),
            mutability,
            members: vec!["a".into(), "b".into()],
        }
    }

    #[test]
    fn pack_mutability_permits_what_it_says() {
        // One table over every variant × both gates, so a new variant has one
        // place to declare itself rather than three tests to be forgotten in.
        let cases = [
            (PackMutability::Locked, false, false),
            (PackMutability::Members, true, false),
            (PackMutability::Full, true, true),
        ];
        for (mutability, members, identity) in cases {
            let p = pack(mutability);
            assert_eq!(
                p.can_edit_members(),
                members,
                "{mutability:?} member editability"
            );
            assert_eq!(
                p.can_edit_identity(),
                identity,
                "{mutability:?} identity editability"
            );
        }
    }

    #[test]
    fn adding_a_member_twice_is_idempotent() {
        let mut p = pack(PackMutability::Full);
        p.add("c".into()).unwrap();
        p.add("c".into()).unwrap();
        assert_eq!(p.members, vec!["a", "b", "c"]);

        // Re-adding an existing member is not a reorder.
        p.add("a".into()).unwrap();
        assert_eq!(p.members, vec!["a", "b", "c"]);
    }

    #[test]
    fn removing_an_absent_member_is_not_an_error() {
        let mut p = pack(PackMutability::Full);
        p.remove("nope").unwrap();
        assert_eq!(p.members, vec!["a", "b"]);
    }

    #[test]
    fn a_locked_pack_rejects_both_kinds_of_edit() {
        let mut p = pack(PackMutability::Locked);
        assert!(p.add("c".into()).is_err());
        assert!(p.remove("a").is_err());
        assert!(p.ensure_identity_editable().is_err());
        // The rejected edits changed nothing.
        assert_eq!(p.members, vec!["a", "b"]);
    }

    #[test]
    fn favorites_takes_members_but_not_a_rename() {
        let mut p = pack(PackMutability::Members);
        p.add("c".into()).unwrap();
        assert_eq!(p.members, vec!["a", "b", "c"]);
        assert!(p.ensure_identity_editable().is_err());
    }

    #[test]
    fn reorder_moves_a_member_and_clamps_the_index() {
        let mut p = pack(PackMutability::Full);
        p.add("c".into()).unwrap();
        p.reorder("c", 0).unwrap();
        assert_eq!(p.members, vec!["c", "a", "b"]);

        p.reorder("c", 99).unwrap();
        assert_eq!(p.members, vec!["a", "b", "c"]);

        assert!(p.reorder("missing", 0).is_err());
    }

    #[test]
    fn retain_members_drops_ghosts_and_reports_whether_it_did() {
        let mut p = pack(PackMutability::Locked);
        assert!(p.retain_members(|m| m != "b"));
        assert_eq!(p.members, vec!["a"]);
        // Nothing left to drop.
        assert!(!p.retain_members(|_| true));
    }

    #[test]
    fn malformed_pack_color_is_rejected() {
        for bad in ["#xyz", "ff0000", "#ff00", "#gggggg", "", "#1234567"] {
            assert!(
                validate_color(bad, "c").is_err(),
                "`{bad}` should be rejected"
            );
        }
        for good in ["#ff0000", "#ff0000aa", "#FFAA33"] {
            assert!(
                validate_color(good, "c").is_ok(),
                "`{good}` should be accepted"
            );
        }
    }

    #[test]
    fn pack_icon_must_be_collection_qualified() {
        for bad in ["star", "", ":star", "fa6-solid:"] {
            assert!(validate_icon(bad).is_err(), "`{bad}` should be rejected");
        }
        assert!(validate_icon("fa6-solid:star").is_ok());
        // An icon the renderer lacks is still shape-valid — it falls back at
        // render time rather than being rejected at import.
        assert!(validate_icon("some-collection:nonexistent").is_ok());
    }

    #[test]
    fn a_shipped_pack_must_name_a_renderable_icon() {
        let mut p = pack(PackMutability::Locked);
        assert!(validate_shipped_pack(&p).is_ok());
        p.icon = "some-collection:nonexistent".into();
        assert!(validate_shipped_pack(&p).is_err());
    }
}
