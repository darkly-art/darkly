# Test-fixture fonts

Fonts here are used **only** by the crate's tests (`include_bytes!` from
`tests/` and `src/text/mod.rs` unit tests). They are not embedded into the
shipped binary.

## Cantarell-VF.otf

- **Family:** Cantarell
- **Why:** a genuine variable font (CFF2, `wght` 100-800 axis) with Latin
  glyphs: exercises variable-axis weight resolution (the Phase-0 spike),
  `register_font`, and `.darkly` font embedding round-tripping.
- **License:** SIL Open Font License, Version 1.1 (OFL-1.1)
- **Copyright:** © The Cantarell Authors
- **Source:** <https://gitlab.gnome.org/GNOME/cantarell-fonts>
- **Full license text:** <https://openfontlicense.org/open-font-license-official-text/>
