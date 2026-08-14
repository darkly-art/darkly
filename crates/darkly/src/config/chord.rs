//! Binding grammar and chord rendering.
//!
//! A binding in `presets/*.yaml` is an optional `site@scope@brush:` prefix
//! followed by a chord. The prefix says *where* the binding applies; the chord
//! says which keys or mouse gesture triggers it. Rust owns both halves because
//! the metadata export ships chords already rendered, and a second
//! implementation of this table on the consumer side is exactly what the
//! artifact exists to avoid.

/// Which modifier vocabulary a chord renders with. Documentation carries both,
/// because a static document is read on both.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Mac,
    Other,
}

/// The parsed halves of a binding: its optional prefix parts and the chord.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParsedBinding {
    /// Binding-site name (`"layerPanel"`, `"canvas"`), or `None` for any.
    pub site: Option<String>,
    /// Active-tool group (`"paint"`, `"select"`), or `None` for any tool.
    pub scope: Option<String>,
    /// Brush kind (`"clone"`), or `None` for any brush.
    pub brush: Option<String>,
    /// Everything after the first `:`, verbatim.
    pub chord: String,
}

/// Split a binding into its prefix parts and its chord.
///
/// The colon is the chord separator; the part before it splits on `@` into
/// `site@scope@brush`, each optional. Anything after the *first* colon is the
/// chord verbatim, so a `@` inside a chord stays put.
///
/// ```text
/// "Delete"                        → site None,     scope None,    brush None,    chord "Delete"
/// "layerPanel:Delete"             → site layerPanel                              chord "Delete"
/// "canvas@paint:shift+drag"       → site canvas,   scope paint,                  chord "shift+drag"
/// "@paint:KeyB"                   →                scope paint,                  chord "KeyB"
/// "canvas@paint@clone:$mod+drag"  → site canvas,   scope paint,   brush clone,   chord "$mod+drag"
/// ```
pub fn parse_binding(raw: &str) -> ParsedBinding {
    let Some(colon) = raw.find(':') else {
        return ParsedBinding {
            chord: raw.to_string(),
            ..Default::default()
        };
    };
    let (prefix, rest) = raw.split_at(colon);
    let chord = rest[1..].to_string();

    let some = |s: &str| (!s.is_empty()).then(|| s.to_string());

    match prefix.find('@') {
        None => ParsedBinding {
            site: some(prefix),
            scope: None,
            brush: None,
            chord,
        },
        Some(at) => {
            let (site, tail) = prefix.split_at(at);
            let mut parts = tail[1..].split('@');
            ParsedBinding {
                site: some(site),
                scope: parts.next().and_then(some),
                brush: parts.next().and_then(some),
                chord,
            }
        }
    }
}

/// Render a chord for one platform's modifier vocabulary.
///
/// Handles both the keyboard vocabulary (`Shift`/`Alt` capitalized, key codes
/// like `KeyA` / `Comma`) and the mouse vocabulary (`shift`/`alt`/`ctrl`/`meta`
/// lowercase, verbs like `click` / `drag`). A part it does not recognize passes
/// through unchanged, which is what keeps a new key code from rendering blank.
pub fn format_chord(chord: &str, platform: Platform) -> String {
    let mac = platform == Platform::Mac;
    chord
        .split('+')
        .map(|part| match part {
            "$mod" => if mac { "⌘" } else { "Ctrl" }.to_string(),
            "Shift" | "shift" => if mac { "⇧" } else { "Shift" }.to_string(),
            "Alt" | "alt" => if mac { "⌥" } else { "Alt" }.to_string(),
            "ctrl" => if mac { "⌃" } else { "Ctrl" }.to_string(),
            "meta" => if mac { "⌘" } else { "Win" }.to_string(),
            "click" => "click".to_string(),
            "doubleClick" => "double-click".to_string(),
            "middleClick" => "middle-click".to_string(),
            "drag" => "drag".to_string(),
            "middleDrag" => "middle-drag".to_string(),
            "rightDrag" => "right-drag".to_string(),
            "Delete" => "Del".to_string(),
            "Comma" => ",".to_string(),
            "Period" => ".".to_string(),
            "Semicolon" => ";".to_string(),
            "Quote" => "'".to_string(),
            "BracketLeft" => "[".to_string(),
            "BracketRight" => "]".to_string(),
            "Backslash" => "\\".to_string(),
            "Minus" => "-".to_string(),
            "Equal" => "=".to_string(),
            "Slash" => "/".to_string(),
            "Backquote" => "`".to_string(),
            other => match other.strip_prefix("Key") {
                Some(k) => k.to_string(),
                None => other.to_string(),
            },
        })
        .collect::<Vec<_>>()
        .join("+")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(
        site: Option<&str>,
        scope: Option<&str>,
        brush: Option<&str>,
        chord: &str,
    ) -> ParsedBinding {
        ParsedBinding {
            site: site.map(str::to_string),
            scope: scope.map(str::to_string),
            brush: brush.map(str::to_string),
            chord: chord.to_string(),
        }
    }

    /// The prefix is a three-part grammar, not one opaque string. Collapsing it
    /// to a `(scope, chord)` pair would report `layerThumb` — a *site* — as a
    /// scope and drop `brush` entirely.
    #[test]
    fn parse_binding_splits_site_scope_brush() {
        let cases: &[(&str, ParsedBinding)] = &[
            ("KeyI", parsed(None, None, None, "KeyI")),
            ("Delete", parsed(None, None, None, "Delete")),
            (
                "layerThumb:alt+click",
                parsed(Some("layerThumb"), None, None, "alt+click"),
            ),
            (
                "layerPanel:Delete",
                parsed(Some("layerPanel"), None, None, "Delete"),
            ),
            (
                "canvas@paint:shift+drag",
                parsed(Some("canvas"), Some("paint"), None, "shift+drag"),
            ),
            ("@paint:KeyB", parsed(None, Some("paint"), None, "KeyB")),
            (
                "canvas@paint@clone:$mod+drag",
                parsed(Some("canvas"), Some("paint"), Some("clone"), "$mod+drag"),
            ),
            // Only the FIRST colon separates; an `@` after it belongs to the chord.
            ("canvas:a@b", parsed(Some("canvas"), None, None, "a@b")),
        ];
        for (raw, want) in cases {
            assert_eq!(&parse_binding(raw), want, "parsing `{raw}`");
        }
    }

    #[test]
    fn format_chord_renders_both_platforms() {
        let cases: &[(&str, &str, &str)] = &[
            ("$mod+KeyZ", "⌘+Z", "Ctrl+Z"),
            ("$mod+Shift+KeyP", "⌘+⇧+P", "Ctrl+Shift+P"),
            ("Alt+KeyA", "⌥+A", "Alt+A"),
            ("alt+click", "⌥+click", "Alt+click"),
            ("ctrl+drag", "⌃+drag", "Ctrl+drag"),
            ("meta+click", "⌘+click", "Win+click"),
            ("$mod+drag", "⌘+drag", "Ctrl+drag"),
            ("shift+doubleClick", "⇧+double-click", "Shift+double-click"),
            ("middleClick", "middle-click", "middle-click"),
            ("middleDrag", "middle-drag", "middle-drag"),
            ("rightDrag", "right-drag", "right-drag"),
            // The twelve key codes.
            ("Delete", "Del", "Del"),
            ("Comma", ",", ","),
            ("Period", ".", "."),
            ("Semicolon", ";", ";"),
            ("Quote", "'", "'"),
            ("BracketLeft", "[", "["),
            ("BracketRight", "]", "]"),
            ("Backslash", "\\", "\\"),
            ("Minus", "-", "-"),
            ("Equal", "=", "="),
            ("Slash", "/", "/"),
            ("Backquote", "`", "`"),
            // Unrecognized parts pass through rather than rendering blank.
            ("Space", "Space", "Space"),
            ("F5", "F5", "F5"),
        ];
        for (chord, mac, other) in cases {
            assert_eq!(&format_chord(chord, Platform::Mac), mac, "mac `{chord}`");
            assert_eq!(
                &format_chord(chord, Platform::Other),
                other,
                "other `{chord}`"
            );
        }
    }
}
