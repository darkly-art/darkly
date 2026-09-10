# Bundled fonts

These fonts are embedded into the engine binary (`include_bytes!`) and registered
into the font collection at startup so text renders identically on every
platform; see `crates/darkly/src/text/mod.rs`.

## NotoSans-VF.ttf / NotoSans-Italic-VF.ttf

- **Family:** Noto Sans (upright + italic variable faces, `wght`/`wdth` axes)
- **License:** SIL Open Font License, Version 1.1 (OFL-1.1)
- **Copyright:** © The Noto Project Authors
- **Source:** <https://github.com/notofonts/latin-greek-cyrillic>
- **Full license text:** <https://openfontlicense.org/open-font-license-official-text/>

The OFL permits bundling and redistribution (including embedding) provided this
attribution travels with the font and the font is not sold on its own.
