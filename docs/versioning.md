# Versioning

Darkly's version is `git describe --tags --long` off the `v*` tags, derived at
build time. It is not stored anywhere in the repo: the `version` fields in
`Cargo.toml` and `package.json` are vestigial and pinned at a deliberately
impossible `0.0.0`, so an artifact carrying one is recognisably unstamped.

```
v0.7.0-2-g1eabe67
 tag    |      SHA
    commits since tag
```

## Commands

```bash
# What version would a build from this checkout bake?
git describe --tags --long

# Write the release's entry (its PRs, by title) to crates/darkly/releases.json,
# then refill the metainfo from it. Needs git and an authenticated gh.
cargo releases --fetch 0.8.0 && cargo sync-docs

# Cut a release: fires publish.yml (crates.io) and docs-artifact.yml, which
# creates the draft GitHub release with the notes from that entry.
git tag -a v0.8.0 -m v0.8.0 && git push origin v0.8.0

# Verify the baked Rust constant matches live git (also runs in the suite).
cargo test -p darkly --lib version_tests

# Force a re-stamp if the constant went stale (a `git gc` repack can do it).
cargo clean -p darkly

# What build wrote this document?
unzip -p painting.darkly manifest.json | jq -r .writer.version
```

The frontend's version is in the About modal, copyable.

## Cutting a release

A release is the commits between the previous `v*` tag and the one being cut,
and its notes are the titles of the PRs merged in that range, read from the
merge subjects. Nothing names a branch or reads a PR body. In order:

1. On `dev`, open the PR into `master` as usual. Its title and body are
   free-form; nothing reads them.
2. `cargo releases --fetch X.Y.Z`, then `cargo sync-docs`. Read the diff of
   `crates/darkly/releases.json` and the metainfo. If a line reads badly, fix
   that PR's title on GitHub and re-run; never edit the snapshot. The fetch
   refuses a title carrying a URL or an en or em dash, which the store listing
   cannot carry.
3. Commit and push `dev`. The `packaging` CI job checks that the snapshot is
   ahead of the last tag; the PR's first run, before this commit, is red on
   that step by design.
4. `git tag -a vX.Y.Z -m vX.Y.Z && git push origin vX.Y.Z`. `publish.yml` and
   `docs-artifact.yml` fire; the latter creates the draft release with the
   notes from the tagged tree.
5. Merge the PR. `master`'s merge commit now contains the tag.
6. Publish the release when the desktop bundles land. Until then the
   `<url type="details">` in the metainfo is a 404; Flathub's manifest bump
   follows the publish, so no user sees it.

An entry is written once; the tag freezes it, and a PR renamed afterwards
changes nothing. If the tag is pushed before step 2, the tagged tree ships
without its entry: delete and re-push the tag before anything is published. A
PR merged into `dev` between steps 2 and 4 is in the tag and not in the entry;
re-run step 2 or merge after the tag. `master` is never merged into `dev`, a
fix ships through `dev` like everything else, which is what keeps a release
PR's own merge out of the next range. The entry's date is the GitHub release's
publish date when it is already published, else the day of the fetch.

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
- **`desktop/package.json`'s version is written by the release build, not by
  hand.** Electron's packager stamps installer metadata from it and reads no
  other source, so `build.sh` overwrites it from the same describe, builds, and
  restores the file. Packaging the desktop host directly, without that script,
  therefore produces installers labelled `0.0.0`: that is the tell, not a bug to
  work around by typing a real-looking number into the file. It sat at `0.6.0`
  for several releases after a stamped value was committed by accident, which
  made unstamped artifacts indistinguishable from a genuine 0.6.0 build.
  `package-lock.json` carries the same placeholder; npm does not check the root
  version when installing, so the two only need to agree for the reader's sake.
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
