//! The icons a brush pack may wear.
//!
//! Declared here as quoted string literals so the frontend's offline icon
//! bundle generator (`frontend/scripts/gen-icon-bundle.mjs`, which scans this
//! crate alongside the TypeScript and Svelte sources) picks them up. An
//! Iconify name that never appears as a literal in this repository is not in
//! the bundle, and the renderer has no network client to fall back on — it
//! would draw nothing at all.
//!
//! Shipped packs must name one of these, which a test in
//! [`crate::brush::packs`] enforces. The pack editor offers exactly this list,
//! so a painter cannot pick an icon that will not render either.

/// `(iconify name, display label)` — the shape the icon-picker widget already
/// consumes, so exposing this list needs no second representation.
pub const PACK_ICONS: &[(&str, &str)] = &[
    ("mdi:brush", "Brush"),
    ("mdi:pencil", "Pencil"),
    ("mdi:water", "Water"),
    ("mdi:blur", "Blur"),
    ("mdi:spray", "Spray"),
    ("mdi:fountain-pen-tip", "Pen"),
    ("mdi:eraser", "Eraser"),
    ("mdi:palette", "Palette"),
    ("mdi:leaf", "Leaf"),
    ("mdi:fire", "Fire"),
    ("mdi:snowflake", "Snowflake"),
    ("mdi:weather-cloudy", "Cloud"),
    ("mdi:shimmer", "Shimmer"),
    ("mdi:diamond-stone", "Gem"),
    ("mdi:dots-horizontal", "Dots"),
    ("mdi:grain", "Grain"),
    ("mdi:texture-box", "Texture"),
    ("mdi:vector-curve", "Curve"),
    ("mdi:shape", "Shape"),
    ("mdi:image-filter-vintage", "Vintage"),
    ("fa6-solid:star", "Star"),
    ("fa6-solid:heart", "Heart"),
    ("fa6-solid:flask", "Flask"),
    ("fa6-solid:folder", "Folder"),
];

/// Drawn in place of an icon the renderer does not have — a pack that arrived
/// in an archive may name anything at all. Must itself be in [`PACK_ICONS`].
pub const PACK_ICON_FALLBACK: &str = "fa6-solid:folder";

/// Whether `name` is an icon a pack may wear.
pub fn is_pack_icon(name: &str) -> bool {
    PACK_ICONS.iter().any(|(n, _)| *n == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fallback_is_itself_a_pack_icon() {
        // Otherwise the glyph drawn when an icon is missing would itself be
        // missing.
        assert!(is_pack_icon(PACK_ICON_FALLBACK));
    }

    #[test]
    fn pack_icons_are_collection_qualified_and_unique() {
        let mut seen: Vec<&str> = Vec::new();
        for (name, label) in PACK_ICONS {
            assert!(
                name.contains(':'),
                "`{name}` is not `collection:name` qualified"
            );
            assert!(!label.is_empty(), "`{name}` has no label");
            assert!(!seen.contains(name), "`{name}` is listed twice");
            seen.push(name);
        }
    }
}
