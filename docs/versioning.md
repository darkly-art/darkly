# Versioning

Darkly's version is `git describe --tags --long` off the `v*` tags, derived at
build time. It is not stored anywhere in the repo: the `version` fields in
`Cargo.toml` and `package.json` sit at `0.1.0` and are vestigial.

```
v0.7.0-2-g1eabe67
 tag    |      SHA
    commits since tag
```

## Commands

```bash
# What version would a build from this checkout bake?
git describe --tags --long

# Cut a release: fires publish.yml (crates.io) and docs-artifact.yml.
git tag -a v0.8.0 -m v0.8.0 && git push origin v0.8.0

# Verify the baked Rust constant matches live git (also runs in the suite).
cargo test -p darkly --lib version_tests

# Force a re-stamp if the constant went stale (a `git gc` repack can do it).
cargo clean -p darkly

# What build wrote this document?
unzip -p painting.darkly manifest.json | jq -r .writer.version
```

The frontend's version is in the About modal, copyable.

## Where it comes from

Cargo and Vite share no runtime, so the derivation exists twice. Each file names
the other its **canonical twin**: a documented exception to the [DRY
Principle](../CONTRIBUTING.md#dry-principle). Change one, change the other.

| | Derives in | Exposed as | Import from |
| --- | --- | --- | --- |
| Rust | [`build.rs`](../crates/darkly/build.rs) | `DARKLY_VERSION` env | [`darkly::VERSION`](../crates/darkly/src/lib.rs) |
| Frontend | [`vite.config.ts`](../frontend/vite.config.ts) | `__DARKLY_VERSION__` | [`darklyVersion`](../frontend/src/version.ts) |

Both fall back to `0.0.0-0-gunknown` when describe fails.

## Rules

- **Never read `env!("CARGO_PKG_VERSION")`** or a `package.json` version. Import
  from the two homes above. A test fails if the crate reverts to it.
- **Never re-derive the string.** No third `git describe` call.
- **Any CI job that builds needs `fetch-depth: 0`.** A shallow checkout has no
  tags, so the build silently ships `0.0.0-0-gunknown`.
- **`--long` and no `--always` are deliberate.** The height and SHA are always
  present, so two builds off the same tag never render alike; and a tagless
  checkout throws through to the fallback instead of degrading to a bare SHA.
- `darkly-macros` publishes before `darkly` and at the same version: `darkly`
  pins it by path, so they move in lockstep.

Save-file compatibility is a separate axis: `container_version` and `requires`
in [`format/manifest.rs`](../crates/darkly/src/format/manifest.rs), under [No
Migrations](../CONTRIBUTING.md#no-migrations--no-backwards-compatibility-pre-release).
The app version rides along in a saved file only as a breadcrumb.
