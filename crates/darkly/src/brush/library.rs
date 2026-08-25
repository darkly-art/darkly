//! The brush library — every brush that exists, and every pack that groups
//! them.
//!
//! "Which brushes exist" and "which packs exist" are one logical concept, so
//! one type owns both. Membership lives on the pack (see
//! [`crate::brush::pack`]); nothing on a brush records which packs hold it,
//! and a brush may be in any number of them.
//!
//! The library is **process-global**, like [`crate::config`]: one library
//! serves every canvas handle on the shared device, so a brush saved in one
//! tab is immediately visible in the next. Reach it through [`with`] and
//! [`with_mut`].
//!
//! It is not document, session or compositor state in the Document Authority
//! sense — a pack belongs to no canvas, never rides a `.darkly` file, and is
//! not derivable from one. It is library state, and the library is exactly one
//! thing.

use std::cell::RefCell;
use std::collections::HashMap;

use indexmap::IndexMap;

use super::metadata::Brush;
use crate::brush::pack::{validate_pack, BrushId, BrushPack, PackId, PackMutability};
use crate::brush::pack_file::PackFile;

/// Summary info for listing brushes without loading the full graph.
#[derive(Clone, Debug, serde::Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub struct BrushInfo {
    /// Opaque identity — what pack member lists and recents hold.
    pub id: String,
    /// Display name, and the engine's public lookup key.
    pub name: String,
    pub author: String,
    pub description: String,
    pub tags: Vec<String>,
    /// Iconify icon shown in place of the baked dab/stroke thumbnails —
    /// present when the graph contains a content-dependent node whose
    /// preview bake renders blank (clone, blur, smudge, liquify). See
    /// [`crate::brush::graph_capabilities`].
    pub icon: Option<&'static str>,
    /// Whether the painter may rename or delete this brush, so the UI can grey
    /// out affordances it would otherwise offer. A hint, not the authority —
    /// same contract as [`BrushPackInfo::can_edit_members`].
    pub can_edit: bool,
}

impl From<&Brush> for BrushInfo {
    fn from(b: &Brush) -> Self {
        let p = &b.metadata;
        BrushInfo {
            id: p.id.clone(),
            name: p.name.clone(),
            author: p.author.clone(),
            description: p.description.clone(),
            tags: p.tags.clone(),
            icon: crate::brush::graph_capabilities(&p.graph).preview_fallback_icon,
            can_edit: b.can_edit(),
        }
    }
}

/// A pack as the UI sees it.
#[derive(Clone, Debug, serde::Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub struct BrushPackInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub icon: String,
    pub primary: String,
    pub secondary: String,
    /// Member brush ids, in the pack's order. The authority on membership —
    /// nothing on [`BrushInfo`] repeats it.
    pub members: Vec<String>,
    /// What the painter may change, so the UI can grey out affordances it
    /// would otherwise offer. A hint, not the authority — the engine rejects a
    /// forbidden edit regardless of what the UI believed.
    pub can_edit_members: bool,
    pub can_edit_identity: bool,
}

impl From<&BrushPack> for BrushPackInfo {
    fn from(p: &BrushPack) -> Self {
        BrushPackInfo {
            id: p.id.clone(),
            name: p.name.clone(),
            description: p.description.clone(),
            icon: p.icon.clone(),
            primary: p.primary.clone(),
            secondary: p.secondary.clone(),
            members: p.members.clone(),
            can_edit_members: p.can_edit_members(),
            can_edit_identity: p.can_edit_identity(),
        }
    }
}

/// Brushes and packs, in one round trip.
///
/// One call rather than two so the two halves cannot disagree: independent
/// `await`s either side of a mutation can, and a member id pointing at a brush
/// the caller has not heard of is exactly the inconsistency this avoids.
#[derive(Clone, Debug, serde::Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub struct LibrarySnapshot {
    pub brushes: Vec<BrushInfo>,
    pub packs: Vec<BrushPackInfo>,
}

/// Every brush and pack in the process.
pub struct BrushLibrary {
    /// Keyed by id. One keyspace, not two — a parallel name map would be the
    /// same fact in two places, and a linear name scan over a few dozen
    /// brushes is free.
    brushes: IndexMap<BrushId, Brush>,
    /// Insertion-ordered so shipped packs keep their declared order and the
    /// painter's own land after them.
    packs: IndexMap<PackId, BrushPack>,
    /// In-memory dab thumbnails for the picker tiles. Not part of any archive
    /// — purely a render cache, rebuilt on theme change alongside the stroke
    /// thumbnails on each [`Brush`].
    dab_thumbnails: HashMap<BrushId, Vec<u8>>,
}

impl BrushLibrary {
    pub fn new() -> Self {
        BrushLibrary {
            brushes: IndexMap::new(),
            packs: IndexMap::new(),
            dab_thumbnails: HashMap::new(),
        }
    }

    /// The shipped library: every built-in brush, then every built-in pack.
    ///
    /// Panics if a shipped pack names a brush that does not exist — that is a
    /// typo in data we control, caught at startup rather than surfacing later
    /// as a pack that renders one brush short.
    pub fn builtin() -> Self {
        let mut lib = BrushLibrary::new();
        for brush in crate::brush::builtin_brushes::all() {
            lib.insert(brush);
        }
        for pack in crate::brush::packs::all() {
            for member in &pack.members {
                assert!(
                    lib.brushes.contains_key(member),
                    "shipped pack '{}' names brush '{member}', which does not exist",
                    pack.id
                );
            }
            lib.packs.insert(pack.id.clone(), pack);
        }
        lib
    }

    // ---- brushes ----

    /// Every brush, sorted by name.
    pub fn list(&self) -> Vec<BrushInfo> {
        let mut infos: Vec<BrushInfo> = self.brushes.values().map(BrushInfo::from).collect();
        infos.sort_by(|a, b| a.name.cmp(&b.name));
        infos
    }

    /// Brushes and packs together — see [`LibrarySnapshot`].
    pub fn snapshot(&self) -> LibrarySnapshot {
        LibrarySnapshot {
            brushes: self.list(),
            packs: self.pack_infos(),
        }
    }

    pub fn get(&self, id: &str) -> Option<&Brush> {
        self.brushes.get(id)
    }

    /// Look a brush up by its display name — the engine's public lookup key.
    pub fn by_name(&self, name: &str) -> Option<&Brush> {
        self.brushes.values().find(|b| b.name() == name)
    }

    /// The id of the brush displayed as `name`.
    pub fn id_for_name(&self, name: &str) -> Option<&str> {
        self.by_name(name).map(|b| b.id())
    }

    /// Add or replace a brush, keyed by its id.
    pub fn insert(&mut self, brush: Brush) {
        self.brushes.insert(brush.metadata.id.clone(), brush);
    }

    /// Reject a name already spoken for by a *different* brush.
    ///
    /// Names are the engine's public lookup key (`by_name`), so two brushes
    /// sharing one makes `brush_load` ambiguous. `rename` has always enforced
    /// this; saving enforces the same rule so the two cannot disagree.
    pub fn ensure_name_free(&self, id: &str, name: &str) -> Result<(), String> {
        if self
            .brushes
            .values()
            .any(|b| b.id() != id && b.name() == name)
        {
            return Err(format!("a brush named '{name}' already exists"));
        }
        Ok(())
    }

    /// Remove a brush and drop it from the member list of every pack that
    /// holds it, so no pack is left pointing at a ghost.
    ///
    /// Bypasses each pack's member gate deliberately: this is not an edit to
    /// those packs, it is the library declining to name something that no
    /// longer exists.
    pub fn delete_brush(&mut self, id: &str) -> Result<(), String> {
        self.ensure_brush_editable(id)?;
        self.brushes.shift_remove(id);
        self.dab_thumbnails.remove(id);
        for pack in self.packs.values_mut() {
            pack.members.retain(|m| m != id);
        }
        Ok(())
    }

    /// Reject a rename or deletion of a brush that is not the painter's.
    ///
    /// A shipped brush comes back from embedded YAML on the next boot, so an
    /// edit to one would appear to work and then silently undo itself. The
    /// same reasoning locks a shipped pack.
    fn ensure_brush_editable(&self, id: &str) -> Result<(), String> {
        match self.brushes.get(id) {
            None => Err(format!("brush '{id}' not found")),
            Some(b) if !b.can_edit() => Err(format!(
                "brush '{}' is built in and cannot be renamed or deleted",
                b.name()
            )),
            Some(_) => Ok(()),
        }
    }

    /// Rename a brush. No pack and no recents entry is touched, because both
    /// hold ids — that is what having an id is for.
    pub fn rename(&mut self, id: &str, new_name: &str) -> Result<(), String> {
        let new_name = new_name.trim();
        if new_name.is_empty() {
            return Err("a brush needs a name".into());
        }
        self.ensure_brush_editable(id)?;
        self.ensure_name_free(id, new_name)?;
        if let Some(brush) = self.brushes.get_mut(id) {
            brush.metadata.name = new_name.to_string();
        }
        Ok(())
    }

    /// A name not already taken, suffixing `"(2)"`, `"(3)"`, … as needed.
    pub fn unique_brush_name(&self, base: &str) -> String {
        unique(base, |c| self.brushes.values().any(|b| b.name() == c))
    }

    pub fn len(&self) -> usize {
        self.brushes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.brushes.is_empty()
    }

    // ---- thumbnails ----

    /// A brush's baked stroke thumbnail, if one has been baked.
    pub fn thumbnail_png(&self, id: &str) -> Option<&[u8]> {
        self.brushes
            .get(id)
            .and_then(|b| b.thumbnail_png.as_deref())
    }

    /// Attach a freshly-baked stroke PNG. Used by the async bake completion
    /// path — save returns immediately without a thumbnail, and this installs
    /// the PNG once the readback lands on a later frame.
    pub fn set_thumbnail(&mut self, id: &str, png: Vec<u8>) -> bool {
        match self.brushes.get_mut(id) {
            Some(brush) => {
                brush.thumbnail_png = Some(png);
                true
            }
            None => false,
        }
    }

    pub fn dab_thumbnail_png(&self, id: &str) -> Option<&[u8]> {
        self.dab_thumbnails.get(id).map(|v| v.as_slice())
    }

    pub fn set_dab_thumbnail(&mut self, id: &str, png: Vec<u8>) {
        self.dab_thumbnails.insert(id.to_string(), png);
    }

    /// Drop every baked stroke and dab thumbnail. Called on theme change so
    /// the next picker refresh re-bakes against the new palette — without
    /// this, brushes stay frozen at whatever theme they were first viewed
    /// under.
    pub fn clear_thumbnails(&mut self) {
        for brush in self.brushes.values_mut() {
            brush.thumbnail_png = None;
        }
        self.dab_thumbnails.clear();
    }

    // ---- packs ----

    pub fn packs(&self) -> impl Iterator<Item = &BrushPack> {
        self.packs.values()
    }

    pub fn pack(&self, id: &str) -> Option<&BrushPack> {
        self.packs.get(id)
    }

    pub fn pack_infos(&self) -> Vec<BrushPackInfo> {
        self.packs.values().map(BrushPackInfo::from).collect()
    }

    fn pack_mut(&mut self, id: &str) -> Result<&mut BrushPack, String> {
        self.packs
            .get_mut(id)
            .ok_or_else(|| format!("brush pack '{id}' not found"))
    }

    /// A pack name not already taken.
    pub fn unique_pack_name(&self, base: &str) -> String {
        unique(base, |c| self.packs.values().any(|p| p.name == c))
    }

    /// Create a painter-owned pack under a caller-supplied id.
    ///
    /// The id comes from the caller because this crate has no random-number
    /// source and adding one for wasm means the `getrandom/js` dance; the
    /// frontend already has a generator. Rust's job is to reject an empty or
    /// duplicate id, which is deterministic and testable.
    pub fn create_pack(
        &mut self,
        id: &str,
        name: &str,
        description: &str,
        icon: &str,
        primary: &str,
        secondary: &str,
    ) -> Result<(), String> {
        if id.trim().is_empty() {
            return Err("a brush pack needs an id".into());
        }
        if self.packs.contains_key(id) {
            return Err(format!("brush pack '{id}' already exists"));
        }
        validate_pack(name, icon, primary, secondary)?;

        let mut pack = BrushPack::new(id, name.trim(), icon, primary, secondary);
        pack.description = description.to_string();
        self.packs.insert(id.to_string(), pack);
        Ok(())
    }

    /// Change a pack's name, description, icon or colors.
    pub fn edit_pack(
        &mut self,
        id: &str,
        name: &str,
        description: &str,
        icon: &str,
        primary: &str,
        secondary: &str,
    ) -> Result<(), String> {
        validate_pack(name, icon, primary, secondary)?;
        let taken = self
            .packs
            .values()
            .any(|p| p.id != id && p.name == name.trim());
        if taken {
            return Err(format!(
                "a brush pack named '{}' already exists",
                name.trim()
            ));
        }

        let pack = self.pack_mut(id)?;
        pack.ensure_identity_editable()?;
        pack.name = name.trim().to_string();
        pack.description = description.to_string();
        pack.icon = icon.to_string();
        pack.primary = primary.to_string();
        pack.secondary = secondary.to_string();
        Ok(())
    }

    /// Delete a pack. **Its brushes survive** — a pack is a grouping, not a
    /// container, and a member that other packs also list is entirely
    /// unaffected. A brush left in no pack is a reachable, safe state.
    pub fn delete_pack(&mut self, id: &str) -> Result<(), String> {
        self.pack_mut(id)?.ensure_identity_editable()?;
        self.packs.shift_remove(id);
        Ok(())
    }

    /// Copy a brush into a pack. It does not leave any pack it is already in.
    pub fn add_to_pack(&mut self, pack: &str, brush: &str) -> Result<(), String> {
        if !self.brushes.contains_key(brush) {
            return Err(format!("brush '{brush}' not found"));
        }
        self.pack_mut(pack)?.add(brush.to_string())
    }

    pub fn remove_from_pack(&mut self, pack: &str, brush: &str) -> Result<(), String> {
        self.pack_mut(pack)?.remove(brush)
    }

    pub fn reorder_in_pack(&mut self, pack: &str, brush: &str, index: usize) -> Result<(), String> {
        self.pack_mut(pack)?.reorder(brush, index)
    }

    /// Export a pack as `.darkly-brush` bytes, carrying its members' records
    /// in member order.
    pub fn export_pack(&self, id: &str) -> Result<Vec<u8>, String> {
        let pack = self
            .packs
            .get(id)
            .ok_or_else(|| format!("brush pack '{id}' not found"))?;
        let brushes = pack
            .members
            .iter()
            .filter_map(|m| self.brushes.get(m))
            .map(|b| b.metadata.clone())
            .collect();
        PackFile::new(pack, brushes).to_bytes()
    }

    /// Import a `.darkly-brush` archive as a new pack under `id`.
    ///
    /// The pack is **always** new, never merged into or replacing an existing
    /// one — merging risks silently overwriting the painter's edits. Its name
    /// is suffixed if it collides.
    ///
    /// Per brush record: a brush whose id the library already has is
    /// **reused**, and the incoming copy discarded. Re-importing your own
    /// export therefore does not multiply your library, and a friend's pack
    /// containing a brush you already have does not overwrite the edits you
    /// made to it. The tradeoff is deliberate — the sender's version of a
    /// shared brush loses to the recipient's.
    pub fn import_pack(&mut self, id: &str, bytes: &[u8]) -> Result<PackId, String> {
        if id.trim().is_empty() {
            return Err("an imported brush pack needs an id".into());
        }
        if self.packs.contains_key(id) {
            return Err(format!("brush pack '{id}' already exists"));
        }
        let file = PackFile::from_bytes(bytes)?;

        let mut members: Vec<BrushId> = Vec::with_capacity(file.brushes.len());
        for mut metadata in file.brushes {
            if self.brushes.contains_key(&metadata.id) {
                // Already ours: keep our copy, and just join the new pack.
                members.push(metadata.id.clone());
                continue;
            }
            // A new brush whose *name* collides is display-suffixed. Names are
            // display; ids are identity.
            metadata.name = self.unique_brush_name(&metadata.name);
            members.push(metadata.id.clone());
            self.insert(Brush::from_metadata(metadata));
        }

        let name = self.unique_pack_name(&file.name);
        let mut pack = BrushPack::new(id, name, file.icon, file.primary, file.secondary);
        pack.description = file.description;
        pack.mutability = PackMutability::Full;
        pack.members = members;
        self.packs.insert(id.to_string(), pack);
        Ok(id.to_string())
    }
}

impl Default for BrushLibrary {
    fn default() -> Self {
        Self::new()
    }
}

/// `base`, or the first `"base (n)"` that `taken` does not claim.
fn unique(base: &str, taken: impl Fn(&str) -> bool) -> String {
    let base = base.trim();
    if !taken(base) {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{base} ({n})"))
        .find(|candidate| !taken(candidate))
        .expect("an unbounded range always yields a free name")
}

thread_local! {
    /// The process-wide library. `thread_local!` + `RefCell` mirrors
    /// [`crate::config`], which solved the same problem the same way: wasm is
    /// single-threaded, and every canvas handle shares one device.
    static LIBRARY: RefCell<BrushLibrary> = RefCell::new(BrushLibrary::builtin());
}

/// Run `f` against the process-wide brush library.
///
/// **Never call a `&mut self` engine method inside the closure.** The borrow
/// is held for the closure's whole body, and a re-entrant `with`/`with_mut`
/// panics at runtime rather than failing to compile. Clone what you need out
/// and end the borrow first — `brush_load` does exactly that.
pub fn with<R>(f: impl FnOnce(&BrushLibrary) -> R) -> R {
    LIBRARY.with(|lib| f(&lib.borrow()))
}

/// Run `f` against the process-wide brush library, mutably. See [`with`] for
/// the borrow rule.
pub fn with_mut<R>(f: impl FnOnce(&mut BrushLibrary) -> R) -> R {
    LIBRARY.with(|lib| f(&mut lib.borrow_mut()))
}

/// Restore the library to its shipped state. Tests only — the process-global
/// would otherwise carry one test's brushes into the next.
#[cfg(any(test, feature = "testing"))]
pub fn reset_for_test() {
    LIBRARY.with(|lib| *lib.borrow_mut() = BrushLibrary::builtin());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brush;
    use crate::brush::metadata::BrushMetadata;

    fn brush_named(id: &str, name: &str) -> Brush {
        Brush::from_metadata(BrushMetadata::from_graph(id, name, brush::default_graph()))
    }

    fn lib_with_two() -> BrushLibrary {
        let mut lib = BrushLibrary::new();
        lib.insert(brush_named("a", "Alpha"));
        lib.insert(brush_named("b", "Beta"));
        lib
    }

    #[test]
    fn library_insert_list_get() {
        let lib = lib_with_two();
        assert_eq!(lib.len(), 2);

        let list = lib.list();
        assert_eq!(list.len(), 2);
        // Sorted by name.
        assert_eq!(list[0].name, "Alpha");
        assert_eq!(list[1].name, "Beta");

        assert!(lib.get("a").is_some());
        assert!(lib.get("missing").is_none());
        assert_eq!(lib.by_name("Beta").unwrap().id(), "b");
        assert_eq!(lib.id_for_name("Alpha"), Some("a"));
    }

    #[test]
    fn every_shipped_brush_is_in_a_shipped_pack() {
        // Shipped brush YAMLs and shipped pack member lists must agree — this
        // is what catches a typo in a member list, and what makes the brushes
        // catalog's derived grouping total.
        let lib = BrushLibrary::builtin();
        for brush in lib.brushes.values() {
            assert!(
                lib.packs().any(|p| p.contains(brush.id())),
                "shipped brush '{}' is in no shipped pack",
                brush.id()
            );
        }
    }

    #[test]
    fn a_brush_can_be_in_two_packs_at_once() {
        // The invariant the whole design rests on: adding to a pack copies a
        // reference, it does not move the brush.
        let mut lib = lib_with_two();
        lib.create_pack("p1", "One", "", "mdi:brush", "#000000", "#ffffff")
            .unwrap();
        lib.create_pack("p2", "Two", "", "mdi:brush", "#000000", "#ffffff")
            .unwrap();

        lib.add_to_pack("p1", "a").unwrap();
        lib.add_to_pack("p2", "a").unwrap();

        assert!(lib.pack("p1").unwrap().contains("a"));
        assert!(lib.pack("p2").unwrap().contains("a"));
    }

    #[test]
    fn copying_a_locked_packs_brush_into_a_user_pack_is_allowed() {
        // A shipped brush lives in a locked pack, and must still be copyable
        // into any pack the painter makes.
        let mut lib = BrushLibrary::builtin();
        let locked = lib
            .packs()
            .find(|p| !p.can_edit_members())
            .expect("a locked shipped pack");
        let (locked_id, member) = (locked.id.clone(), locked.members[0].clone());

        lib.create_pack("mine", "Mine", "", "mdi:brush", "#000000", "#ffffff")
            .unwrap();
        lib.add_to_pack("mine", &member).unwrap();

        assert!(lib.pack("mine").unwrap().contains(&member));
        // And it did not leave the pack it came from.
        assert!(lib.pack(&locked_id).unwrap().contains(&member));
    }

    #[test]
    fn adding_to_a_locked_pack_is_rejected() {
        let mut lib = BrushLibrary::builtin();
        lib.insert(brush_named("mine", "Mine"));
        let before = lib.pack("basic").unwrap().members.clone();

        assert!(lib.add_to_pack("basic", "mine").is_err());
        assert_eq!(lib.pack("basic").unwrap().members, before);
    }

    #[test]
    fn removing_from_a_locked_pack_is_rejected() {
        let mut lib = BrushLibrary::builtin();
        let before = lib.pack("basic").unwrap().members.clone();

        assert!(lib.remove_from_pack("basic", &before[0]).is_err());
        assert_eq!(lib.pack("basic").unwrap().members, before);
    }

    #[test]
    fn deleting_a_pack_leaves_its_brushes_alone() {
        let mut lib = lib_with_two();
        lib.create_pack("p1", "One", "", "mdi:brush", "#000000", "#ffffff")
            .unwrap();
        lib.create_pack("p2", "Two", "", "mdi:brush", "#000000", "#ffffff")
            .unwrap();
        lib.add_to_pack("p1", "a").unwrap();
        lib.add_to_pack("p2", "a").unwrap();

        lib.delete_pack("p1").unwrap();

        assert!(lib.pack("p1").is_none());
        // The brush survives, and its membership elsewhere is untouched.
        assert!(lib.get("a").is_some());
        assert!(lib.pack("p2").unwrap().contains("a"));
    }

    #[test]
    fn deleting_a_locked_pack_is_rejected() {
        let mut lib = BrushLibrary::builtin();
        assert!(lib.delete_pack("basic").is_err());
        assert!(lib.pack("basic").is_some());
    }

    #[test]
    fn deleting_a_brush_removes_it_from_every_pack() {
        let mut lib = lib_with_two();
        lib.create_pack("p1", "One", "", "mdi:brush", "#000000", "#ffffff")
            .unwrap();
        lib.create_pack("p2", "Two", "", "mdi:brush", "#000000", "#ffffff")
            .unwrap();
        lib.add_to_pack("p1", "a").unwrap();
        lib.add_to_pack("p2", "a").unwrap();

        lib.delete_brush("a").unwrap();

        assert!(lib.get("a").is_none());
        assert!(!lib.pack("p1").unwrap().contains("a"));
        assert!(!lib.pack("p2").unwrap().contains("a"));
        assert!(
            lib.delete_brush("a").is_err(),
            "a brush that is gone cannot be deleted again"
        );
    }

    #[test]
    fn a_shipped_brush_cannot_be_renamed_or_deleted() {
        // It is rebuilt from embedded YAML on the next boot, so either edit
        // would appear to work and then undo itself.
        let mut lib = BrushLibrary::builtin();
        let member = lib.pack("basic").unwrap().members[0].clone();

        assert!(!lib.get(&member).unwrap().can_edit());
        assert!(lib.delete_brush(&member).is_err());
        assert!(lib.rename(&member, "Mine Now").is_err());
        // The rejected edits changed nothing.
        assert!(lib.get(&member).is_some());
        assert!(lib.pack("basic").unwrap().contains(&member));
    }

    #[test]
    fn renaming_a_brush_touches_no_pack() {
        // The payoff of id-keyed membership.
        let mut lib = lib_with_two();
        lib.create_pack("p1", "One", "", "mdi:brush", "#000000", "#ffffff")
            .unwrap();
        lib.add_to_pack("p1", "a").unwrap();
        let before = lib.pack("p1").unwrap().members.clone();

        lib.rename("a", "Renamed").unwrap();

        assert_eq!(lib.pack("p1").unwrap().members, before);
        assert_eq!(lib.get("a").unwrap().name(), "Renamed");
        assert!(lib.by_name("Alpha").is_none());
    }

    #[test]
    fn renaming_onto_a_taken_name_is_rejected() {
        let mut lib = lib_with_two();
        assert!(lib.rename("a", "Beta").is_err());
        assert!(lib.rename("a", "  ").is_err());
        assert!(lib.rename("missing", "Whatever").is_err());
        // Renaming to its own name is a no-op, not a collision.
        lib.rename("a", "Alpha").unwrap();
    }

    #[test]
    fn a_brush_in_no_pack_is_still_listed() {
        // The reachable-orphan state: a brush does not depend on a pack to
        // exist.
        let mut lib = lib_with_two();
        lib.create_pack("p1", "One", "", "mdi:brush", "#000000", "#ffffff")
            .unwrap();
        lib.add_to_pack("p1", "a").unwrap();
        lib.remove_from_pack("p1", "a").unwrap();

        assert!(lib.get("a").is_some());
        assert!(lib.list().iter().any(|b| b.id == "a"));
        assert!(!lib.packs().any(|p| p.contains("a")));
    }

    #[test]
    fn creating_a_pack_rejects_a_duplicate_or_empty_id() {
        let mut lib = BrushLibrary::new();
        lib.create_pack("p1", "One", "", "mdi:brush", "#000000", "#ffffff")
            .unwrap();
        assert!(lib
            .create_pack("p1", "Other", "", "mdi:brush", "#000000", "#ffffff")
            .is_err());
        assert!(lib
            .create_pack("  ", "Other", "", "mdi:brush", "#000000", "#ffffff")
            .is_err());
        // And a malformed color never reaches the library.
        assert!(lib
            .create_pack("p2", "Two", "", "mdi:brush", "not-a-color", "#ffffff")
            .is_err());
    }

    #[test]
    fn editing_a_pack_rejects_a_locked_one_and_a_taken_name() {
        let mut lib = BrushLibrary::builtin();
        assert!(lib
            .edit_pack("basic", "Renamed", "", "mdi:brush", "#000000", "#ffffff")
            .is_err());

        lib.create_pack("p1", "One", "", "mdi:brush", "#000000", "#ffffff")
            .unwrap();
        lib.create_pack("p2", "Two", "", "mdi:brush", "#000000", "#ffffff")
            .unwrap();
        assert!(lib
            .edit_pack("p2", "One", "", "mdi:brush", "#000000", "#ffffff")
            .is_err());

        lib.edit_pack("p2", "Renamed", "d", "mdi:water", "#111111", "#222222")
            .unwrap();
        let p = lib.pack("p2").unwrap();
        assert_eq!(p.name, "Renamed");
        assert_eq!(p.icon, "mdi:water");
    }

    #[test]
    fn adding_a_missing_brush_to_a_pack_is_rejected() {
        let mut lib = BrushLibrary::new();
        lib.create_pack("p1", "One", "", "mdi:brush", "#000000", "#ffffff")
            .unwrap();
        assert!(lib.add_to_pack("p1", "nope").is_err());
        assert!(lib.add_to_pack("nope", "nope").is_err());
    }

    #[test]
    fn pack_export_import_round_trip() {
        let mut lib = lib_with_two();
        lib.create_pack("p1", "Mine", "d", "mdi:water", "#3355ff", "#ffffff")
            .unwrap();
        lib.add_to_pack("p1", "a").unwrap();
        lib.add_to_pack("p1", "b").unwrap();

        let bytes = lib.export_pack("p1").unwrap();

        let mut fresh = BrushLibrary::new();
        fresh.import_pack("new", &bytes).unwrap();

        let pack = fresh.pack("new").unwrap();
        assert_eq!(pack.name, "Mine");
        assert_eq!(pack.icon, "mdi:water");
        assert_eq!(pack.members, vec!["a", "b"], "member order survives");
        // An imported pack is always the painter's own.
        assert!(pack.can_edit_identity());
        assert_eq!(fresh.len(), 2);
    }

    #[test]
    fn importing_a_pack_whose_name_collides_gets_a_suffixed_name() {
        let mut lib = lib_with_two();
        lib.create_pack("p1", "Mine", "", "mdi:brush", "#000000", "#ffffff")
            .unwrap();
        lib.add_to_pack("p1", "a").unwrap();
        let bytes = lib.export_pack("p1").unwrap();

        lib.import_pack("p2", &bytes).unwrap();

        // Both survive; neither was merged into the other.
        assert_eq!(lib.pack("p1").unwrap().name, "Mine");
        assert_eq!(lib.pack("p2").unwrap().name, "Mine (2)");
    }

    #[test]
    fn importing_a_pack_containing_a_known_brush_reuses_it() {
        // Re-importing your own export must not multiply your library, and
        // must not overwrite edits you made to a brush you already have.
        let mut lib = lib_with_two();
        lib.create_pack("p1", "Mine", "", "mdi:brush", "#000000", "#ffffff")
            .unwrap();
        lib.add_to_pack("p1", "a").unwrap();
        let bytes = lib.export_pack("p1").unwrap();

        lib.rename("a", "My Edited Name").unwrap();
        lib.import_pack("p2", &bytes).unwrap();

        assert_eq!(lib.len(), 2, "the library did not grow");
        assert_eq!(
            lib.get("a").unwrap().name(),
            "My Edited Name",
            "the recipient's copy wins"
        );
        assert!(lib.pack("p2").unwrap().contains("a"));
    }

    #[test]
    fn importing_rejects_a_duplicate_pack_id() {
        let mut lib = lib_with_two();
        lib.create_pack("p1", "Mine", "", "mdi:brush", "#000000", "#ffffff")
            .unwrap();
        let bytes = lib.export_pack("p1").unwrap();
        assert!(lib.import_pack("p1", &bytes).is_err());
        assert!(lib.import_pack("", &bytes).is_err());
    }

    #[test]
    fn unique_names_suffix_until_free() {
        let mut lib = lib_with_two();
        assert_eq!(lib.unique_brush_name("Gamma"), "Gamma");
        assert_eq!(lib.unique_brush_name("Alpha"), "Alpha (2)");
        lib.insert(brush_named("a2", "Alpha (2)"));
        assert_eq!(lib.unique_brush_name("Alpha"), "Alpha (3)");
    }

    #[test]
    fn thumbnails_are_keyed_by_id_and_cleared_together() {
        let mut lib = lib_with_two();
        assert!(lib.set_thumbnail("a", vec![1, 2, 3]));
        lib.set_dab_thumbnail("a", vec![4, 5, 6]);
        assert_eq!(lib.thumbnail_png("a"), Some(&[1u8, 2, 3][..]));
        assert_eq!(lib.dab_thumbnail_png("a"), Some(&[4u8, 5, 6][..]));

        assert!(!lib.set_thumbnail("missing", vec![]));

        lib.clear_thumbnails();
        assert!(lib.thumbnail_png("a").is_none());
        assert!(lib.dab_thumbnail_png("a").is_none());
    }

    #[test]
    fn deleting_a_brush_drops_its_dab_thumbnail() {
        let mut lib = lib_with_two();
        lib.set_dab_thumbnail("a", vec![1]);
        lib.delete_brush("a").unwrap();
        assert!(lib.dab_thumbnail_png("a").is_none());
    }

    #[test]
    fn the_snapshot_carries_both_halves() {
        let lib = BrushLibrary::builtin();
        let snap = lib.snapshot();
        assert!(!snap.brushes.is_empty());
        assert!(!snap.packs.is_empty());
        // Every member id in the snapshot resolves to a brush in the same
        // snapshot — the consistency one round trip buys.
        for pack in &snap.packs {
            for member in &pack.members {
                assert!(
                    snap.brushes.iter().any(|b| &b.id == member),
                    "pack '{}' names '{member}', absent from the same snapshot",
                    pack.id
                );
            }
        }
    }
}
