# Fonts used by documentation graphics

These are read from disk by `frontend/scripts/render-doc-graphics.mjs` and handed
to resvg's `fontFiles`. They are not bundled into the app and no app code imports
them.

Only faces the graphics actually set are kept here. resvg cannot decode `woff2`
(`fontdb` reports "malformed font") and ignores `@font-face`, so a graphic's font
has to exist as a `ttf`/`otf` **file path**; that is the only reason these are
`ttf` where the website serves `woff2`.

## oldstyle-regular.ttf

- **Family:** `OldStyle 1` (subfamily `HPLHS`)
- **Copyright:** © 2002 by Andrew H. Leman. Revived from the Linotype Catalog
  exclusively for the HPLHS.
- **Source:** The H. P. Lovecraft Historical Society, <https://www.hplhs.org/>

Set the family as **`OldStyle 1`**, not `Oldstyle HPLHS`. The latter is the name
the website's stylesheet uses, invented when the face was converted to `woff2`;
it does not appear in this file's `name` table. resvg does not error on a family
it cannot match, it falls back to whatever else is loaded, so the wrong name
renders plausibly and only breaks once a second face is loaded.

**Licence:** none accompanies the font. The archive it came from carries no
licence file, and the `name` table declares neither a licence description
(ID 13) nor a licence URL (ID 14). HPLHS offers it as a free download under a
personal, non-transferable licence and sells extended use separately. It is
included here as a deliberate, informed decision rather than under a grant. It is
referenced from exactly one CSS declaration
(`frontend/src/graphics/Veils.svelte`), so substituting an openly licensed
oldstyle face is a one-line change plus a re-render.

## Noto Sans

Graphics set body text in `Noto Sans`, read from
`crates/darkly/resources/fonts/NotoSans-VF.ttf`, which the engine already bundles
under the SIL Open Font License 1.1. See
`crates/darkly/resources/fonts/NOTICE.md`. It is not duplicated here.
