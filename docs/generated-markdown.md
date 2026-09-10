# Generated Markdown

Parts of this repository's markdown are generated from the registries, so a name
or a description has exactly one home: the registration that owns it. Read this
before editing a catalog table in the README, before adding an entry to a
catalog, or when `cargo test` fails on a markdown file you did not touch.

[`CONTRIBUTING.md`](../CONTRIBUTING.md#generated-markdown) carries the one rule
that binds every contributor (never edit inside a region). This file is the
mechanism and the commands. The code side is documented at the top of
[`crates/darkly/src/docs_md/mod.rs`](../crates/darkly/src/docs_md/mod.rs).

## Regions

A file opts a span of itself in by bracketing it with HTML comments, which render
as nothing:

```markdown
<!-- darkly:catalog-table catalog=effects -->
…generated…
<!-- /darkly:catalog-table -->
```

**Never edit inside a region**: the next sync overwrites it. Every name and
description in one is a `&'static str` on the registration that owns it, so a
typo in the README's effects table is fixed in
[`crates/darkly/src/gpu/effects/`](../crates/darkly/src/gpu/effects/).

```bash
cargo sync-docs              # refill every region
cargo sync-docs -- --check   # report drift, write nothing
```

`tests/docs_md.rs` fails if a committed region is stale, so the ordinary test
suite is the gate: run `cargo sync-docs` when you have touched a registration and
it will tell you what it rewrote. A new kind of region is a new file in
[`crates/darkly/src/docs_md/fragments/`](../crates/darkly/src/docs_md/fragments/)
exporting `pub fn register()`: nothing else is touched.

## Preview stills

Preview stills are the one part that is **not** automatic: they need a GPU and
land in the repository as binaries, so they are rendered deliberately when a
catalog gains or loses an entry. `tests/docs_md.rs` fails on a region linking to
an image that is not in the checkout, which is how you find out.

```bash
cargo run --release -p darkly --features testing --bin render_docs -- \
  --stills --catalog effects
```

## Catalog graphics

A `catalog-graphic` region embeds a rendered picture of a catalog instead of a
table of it, as the README's effects section does. The picture is a Svelte
component in [`frontend/src/graphics/`](../frontend/src/graphics/), rasterized by
resvg rather than a browser; what that costs a component is documented at the top
of the runner. `npm test` re-renders each one and fails if the committed image is
stale.

```bash
cargo run -q -p darkly --bin export-docs -- --out target/docs/metadata.json
node frontend/scripts/render-doc-graphics.mjs --metadata target/docs/metadata.json
```
