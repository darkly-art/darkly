# Versioning: one git-derived string, two bake paths

Darkly has exactly one version, and it is not written down anywhere in the
repository. It is derived from git tags at build time, independently on the Rust
side and the frontend side, and every place that displays or stamps a version
reads the result of that derivation.

This file covers the *application* version. Save-file compatibility is a
separate axis with its own rules (`container_version` and `requires` in
[`crates/darkly/src/format/manifest.rs`](../crates/darkly/src/format/manifest.rs)),
governed by [No Migrations / No Backwards
Compatibility](../CONTRIBUTING.md#no-migrations--no-backwards-compatibility-pre-release).

## The tag is the source of truth

The version is `git describe --tags --long`: the latest `v*` tag, the commit
height since it, and the abbreviated SHA, for example `v0.7.0-2-g1eabe67`. At
height 0 (HEAD *is* the tag) it reads `v0.7.0-0-g<sha>`.

`--long` is deliberate: the height and SHA are always present, so a build from a
tagged commit and a build from two commits later can never render as the same
string. That is what makes a pasted version from a bug report actionable.

There is no `--always`. On a tagless or shallow checkout we want describe to
fail so the fallback fires, rather than silently degrading to a bare SHA that
does not parse as a version.

**The fallback is `0.0.0-0-gunknown`**, byte-identical on both sides. It keeps
the describe shape, so consumers and tests need no special case for it.

**The `version` fields in `Cargo.toml` and `package.json` are vestigial.** They
sit at `0.1.0` and are meaningless. `publish.yml` overwrites the crates' values
from the tag at publish time. Never read `env!("CARGO_PKG_VERSION")`; there is a
test that fails if the crate version ever silently reverts to it.

## Two derivations, declared as canonical twins

Cargo and Vite share no runtime, so the same three-line derivation exists twice.
This is a documented exception to the [DRY
Principle](../CONTRIBUTING.md#dry-principle), and each side names the other:

| Side | Derives in | Exposed as | Read through |
| --- | --- | --- | --- |
| Rust | [`crates/darkly/build.rs`](../crates/darkly/build.rs) `emit_darkly_version` | `DARKLY_VERSION` compile-time env | [`darkly::VERSION`](../crates/darkly/src/lib.rs) |
| Frontend | [`frontend/vite.config.ts`](../frontend/vite.config.ts) `gitVersion` | `__DARKLY_VERSION__` define | [`darklyVersion`](../frontend/src/version.ts) |

Change one, change the other. The command and the fallback string must stay
identical, or a saved file will disagree with the About modal of the build that
saved it.

Nothing else may shell out to `git describe` or re-derive a version. Both sides
have exactly one home for the string; import it.

`build.rs` also emits `cargo:rerun-if-changed` hints for `.git/HEAD`,
`.git/packed-refs`, `.git/refs/tags`, and HEAD's own ref, so a new tag or a
checkout re-stamps the constant. It is best-effort by design: a `git gc` repack
can move refs without touching those paths, and a stale constant after one is
not worth a mandatory rebuild on every build. `cargo clean -p darkly` if you
ever see one.

## Where the string surfaces

- **About modal** ([`AboutModal.svelte`](../frontend/src/ui/AboutModal.svelte)):
  shown verbatim and copyable, no decoration, so what an artist pastes into an
  issue is exactly what the build stamps elsewhere.
- **Saved documents**: `ManifestWriter::current()` stamps it into every
  `.darkly` file as an informational breadcrumb of which build wrote it. It is
  never read back for compatibility decisions.
- **Brush bundles**: `default_engine_version()` in
  [`brush/metadata.rs`](../crates/darkly/src/brush/metadata.rs).
- **Docs artifact**: `export-docs` and `render-docs` both stamp it, and it is
  the pairing key that stops a consumer from combining prose, metadata, or a
  preview from mismatched builds.

## Cutting a release

Push a `v*` tag. Two workflows fire from it:

- [`publish.yml`](../.github/workflows/publish.yml) publishes `darkly-macros`
  then `darkly` to crates.io at the tag's version. They move in lockstep because
  `darkly` pins `darkly-macros` through its path dependency, so the macro crate
  must go up first.
- [`docs-artifact.yml`](../.github/workflows/docs-artifact.yml) builds the
  documentation tarball for the release.

**Any CI job that builds Darkly needs `fetch-depth: 0`.** A shallow checkout has
no tags, `git describe` fails, and the build silently ships
`0.0.0-0-gunknown`.

## What the tests guard

On the Rust side, [`lib.rs`](../crates/darkly/src/lib.rs) `version_tests`:

- `version_matches_live_git_describe` is the feature test. It compares the baked
  constant against a live `git describe`, and only asserts equality when the
  command succeeds, so it exercises the real pipeline instead of passing
  vacuously on the fallback in a git-less checkout.
- `version_is_not_cargo_pkg_version` is the regression guard against reverting
  to the stale `Cargo.toml` semver, and also asserts the describe shape.

On the frontend side,
[`version.test.ts`](../frontend/src/__tests__/version.test.ts) asserts the same
shape for `darklyVersion`.
