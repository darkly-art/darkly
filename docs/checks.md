# The Check Suite: why each flag is load-bearing

The commands live in
[`CONTRIBUTING.md`](../CONTRIBUTING.md#lint--ci-checks) and are run from there.
This file records what each flag is defending against, so nobody drops one that
looks redundant and reintroduces the failure it was added for.

## `--features darkly/testing`

Exposes `gpu::test_utils`, `blocking_read()`, and the engine's `test_readback_*`
accessors that integration tests rely on. The feature is the compile-time gate
enforcing [No Blocking GPU
Readbacks](../CONTRIBUTING.md#no-blocking-gpu-readbacks): without it, production
code cannot even name those functions.

## `--test-threads=1`

Mandatory, not a preference. GPU-touching integration tests (`engine.rs`,
`blend_modes.rs`, and friends) share a process-wide wgpu device and SIGSEGV when
run in parallel.

## Both `npx tsc --noEmit` and `npm run check`

`tsc --noEmit` is the TypeScript gate for `.ts` files, but it **cannot see inside
`.svelte` files**: it does not parse the extension, and neither `vite build` nor
Vitest type-checks components. `svelte-check` (`npm run check`) is the only gate
that type-checks `.svelte` scripts and templates, via `svelte2tsx` plus the
TypeScript API, and it catches nonexistent engine methods, wrong props, and
null-safety in components. Both are required: `tsc` alone gives a false green on
component bugs.

## `npm test`

Vitest runs in the node environment by default, so there is no DOM and globals
like `KeyboardEvent`, `PointerEvent` and `window` are undefined. Test against
plain object fakes (`{ key, shiftKey } as KeyboardEvent`), and for code that
touches `window`, stub it with `vi.stubGlobal('window', …)` and a fake node: see
[`src/lib/__tests__/dismiss.test.ts`](../frontend/src/lib/__tests__/dismiss.test.ts).

A file opting in with `// @vitest-environment jsdom` gets a DOM and can `mount`
a component, which is how behaviour that only exists in how a component
sequences DOM reads and writes is covered: see
[`addLayerModal.component.test.ts`](../frontend/src/ui/layers/__tests__/addLayerModal.component.test.ts).
jsdom has no layout, so it computes no geometry, runs no animation frames and
implements neither `showModal` nor scrolling. A component test that needs any of
those stubs them and supplies its own canned rects, which is a decision about
what the test controls rather than a workaround: see
[`brushExplorer.open.component.test.ts`](../frontend/src/ui/brush_explorer/__tests__/brushExplorer.open.component.test.ts).
Anything whose cause is real layout (scroll anchoring, percentage resolution,
`display: none` semantics) cannot be reproduced here at all.

It is also the staleness gate for catalog graphics: it re-renders each one and
fails if the committed image no longer matches its component and stills. See
[generated-markdown.md](generated-markdown.md).

## The docs site: `DOCS_PREVIEWS=0 make docs && (cd docs-site && npm test && npm run test:browser)`

`make docs` exports the registries and the manual, then `npm run build` in
`docs-site/` runs `astro check` and `svelte-check` before `astro build`. Both
type gates, for the reason above: `astro check` covers `.astro` files and TS
modules but passes a type error inside a `.svelte` island, which only
`svelte-check` reports. The build fails on an unknown frontmatter key (the
collection schema is strict) and, in `gen-docs.mjs` before Astro starts, on a
manual page whose `:hotkey[id]` or `:action[id]` names no action or sits in a
heading, naming the file, line and id. A page that otherwise fails to render
does not fail the build: Astro logs the error, ships the page with an empty
article and exits 0, so that is caught by `check-dist.mjs` below, with the
reason in the build log. `DOCS_PREVIEWS=0` skips the preview render, which
needs a GPU and ffmpeg; the release workflow renders them.

`npm test` then runs `check-shared.mjs` (`src/shared/` imports nothing outside
itself, since other sites compile it as source), `check-hotkeys.mjs` (the
hotkey markup and the manual's directives, over a fixture) and `check-dist.mjs`
(every link resolves under `/docs/`, search indexes every page and loads from
under it, no article is empty, every key group indexes only its default
variant, every key reads exactly as `export-docs` rendered it, and no hotkey
directive reaches Astro, whose content cache re-renders a page only when its
file changes and would otherwise keep a key the presets no longer bind).

`npm run test:browser` runs `check-browser.mjs`, the only proof of the client
half: it serves `dist/` and drives headless Chrome or Chromium over the
DevTools protocol through the reader's hotkey choice (no JavaScript, the
detected convention, choosing a preset, no flash across navigation, search
holding only default keys, the header at 320 to 1024 px). It needs
`google-chrome` or `chromium` on `PATH`, or `CHROME` naming the binary, and
fails rather than skipping without one.

## Housekeeping: `cargo sweep`

Not a check, but the reason `target/` balloons. Cargo orphans a ~300 MB static
test binary on every fingerprint change and never garbage-collects it.
`cargo install cargo-sweep` once, then periodically:

```bash
cargo sweep --time 7
```
