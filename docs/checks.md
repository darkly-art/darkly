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

Vitest runs in the node environment, so there is no DOM and globals like
`KeyboardEvent`, `PointerEvent` and `window` are undefined. Test against plain
object fakes (`{ key, shiftKey } as KeyboardEvent`), and for code that touches
`window`, stub it with `vi.stubGlobal('window', …)` and a fake node: see
[`src/lib/__tests__/dismiss.test.ts`](../frontend/src/lib/__tests__/dismiss.test.ts).

It is also the staleness gate for catalog graphics: it re-renders each one and
fails if the committed image no longer matches its component and stills. See
[generated-markdown.md](generated-markdown.md).

## Housekeeping: `cargo sweep`

Not a check, but the reason `target/` balloons. Cargo orphans a ~300 MB static
test binary on every fingerprint change and never garbage-collects it.
`cargo install cargo-sweep` once, then periodically:

```bash
cargo sweep --time 7
```
