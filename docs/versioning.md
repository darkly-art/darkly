# Versioning

Darkly's version is `git describe --tags --long` off the `v*` tags, derived at
build time. It is not stored anywhere in the repo: the `version` fields in
`Cargo.toml` and `package.json` are vestigial and pinned at a deliberately
impossible `0.0.0`, so an artifact carrying one is recognisably unstamped. A
tree with no `.git` reads it from a file `git archive` fills in (below).

```
v0.7.0-2-g1eabe67
 tag    |      SHA
    commits since tag
```

## Commands

```bash
# What version would a build from this checkout bake?
git describe --tags --long

# Cut a release (the whole procedure is below).
scripts/release.sh 0.9.0

# Render the store listing's release history from the tags, as CI and every
# packaging channel do.
scripts/metainfo-releases.sh packaging/art.darkly.Darkly.metainfo.xml > out.xml

# Verify the baked constant matches its source (also runs in the suite).
cargo test -p darkly --lib version

# What version would a release tarball of this commit bake?
git archive HEAD crates/darkly/version.txt | tar -xO

# Force a re-stamp if the constant went stale (a `git gc` repack can do it).
cargo clean -p darkly

# What build wrote this document?
unzip -p painting.darkly manifest.json | jq -r .writer.version
```

The frontend's version is in the About modal, copyable. It is the engine's,
read through the wasm bridge.

## Cutting a release

A release is the commits between the previous `vX.Y.Z` tag and the one being
cut. Its record is the annotated tag: the tag name is the version, the tagger
date is the release date, and each line of the tag body is one change, shown in
the store listing. Nothing is committed before the tag. One commit follows it:
the metainfo keeps a copy of the release history, generated from the tags, and
that copy is refreshed and committed once the new tag exists.

You need: a clean `dev` checkout in sync with `origin/dev`, an authenticated
`gh`, and the open `dev` into `master` release PR green on that commit.

1. **Tag.** `scripts/release.sh X.Y.Z`, with `X.Y.Z` above the last tag
   (`git describe --tags --abbrev=0`). It refuses unless the release PR's
   checks have passed on the commit being tagged; if no release PR is open, it
   opens one and stops, so run it again once that PR is green. It then lists
   the PRs merged since the last tag, bots excluded, one `Title (#N)` per line,
   and asks. On `y` it tags `vX.Y.Z` with those lines as the body, pushes the
   tag, retitles the release PR `Dev -> Master X.Y.Z` with a link to each of
   those PRs as its body, merges it, and refreshes the `<releases>` block in
   `packaging/art.darkly.Darkly.metainfo.xml`. If a line reads badly, answer
   `n`, fix that PR's title on GitHub, and run it again. It refuses a title
   carrying a URL, which the store listing cannot carry.
2. **Commit the release history.**

   ```bash
   git commit packaging/art.darkly.Darkly.metainfo.xml -m "Release history for vX.Y.Z"
   git push origin dev
   ```

   The commit reaches `master` with the next release PR. CI validates the file
   but never fails for it being behind the tags; channels refill the block from
   the tags at build time regardless.

Everything else happens on its own. The tag push runs `publish.yml`
(publishes `darkly-macros` and `darkly` to crates.io at `X.Y.Z`) and
`docs-artifact.yml` (creates the draft pre-release `vX.Y.Z` with GitHub's
generated notes and attaches the documentation tarball). A separate, private
pipeline that holds the signing keys picks up the new tag within a few hours,
builds and smoke-tests the signed desktop bundles, attaches them to the draft,
and publishes the release once every platform has passed. Until then the
metainfo's `<url type="details">` points at a draft, and is a 404 to users.

Flathub is not live yet. When it is, its manifest pins the tag and commit and
is bumped after the release is published.

**Notes by hand.** The tag body is what users read in the store, and PR titles
are commit language. To write it yourself, skip the script: `git tag -a
vX.Y.Z`, subject `vX.Y.Z`, then a blank line and one change per line; `git
push origin vX.Y.Z`; merge the release PR with the PR links as its body;
then refresh the metainfo and carry on from step 2:

```bash
scripts/metainfo-releases.sh packaging/art.darkly.Darkly.metainfo.xml > /tmp/metainfo.xml
cat /tmp/metainfo.xml > packaging/art.darkly.Darkly.metainfo.xml
```

A tag with no body still releases, with no description in the store.

**A bad tag.** Before the release is published only: `git tag -d vX.Y.Z && git push origin
:refs/tags/vX.Y.Z`, discard the refresh if it is not committed yet (`git
checkout packaging/art.darkly.Darkly.metainfo.xml`), then steps 1 and 2 again.
The re-push is safe: `publish.yml` skips a version already on crates.io, and
`docs-artifact.yml` re-attaches to the existing draft.

`master` is never merged into `dev`; a fix ships through `dev` like everything
else, which keeps a release PR's own merge out of the next range.

### The release PR

Its body is an unordered list of the release's PR links, one per line,
numerically sorted, and nothing else:

```markdown
- https://github.com/darkly-art/darkly/pull/117
- https://github.com/darkly-art/darkly/pull/119
```

It is the one exception to the two-part PR description in CONTRIBUTING.md,
because nothing reads this body; the release's record is the tag.
`scripts/release.sh` writes it.

## Where it comes from

One derivation, in [`build.rs`](../crates/darkly/build.rs), exposed as
[`darkly::VERSION`](../crates/darkly/src/lib.rs). The frontend reads it from
the wasm bridge's `version()`; there is no second derivation. The first of
these that applies wins:

1. **`crates/darkly/version.txt`, when filled in.** The file holds
   `$Format:...$` placeholders that `git archive` substitutes, because
   `.gitattributes` marks it `export-subst`. GitHub's release tarballs are
   `git archive` output, and `publish.yml` fills it the same way before
   `cargo publish`, so a release tarball and the crates.io package both carry
   the commit they came from. git's `%(describe)` has no `--long`, so at a tag
   the file says `v0.9.0 <sha> <date>` and
   [`version.rs`](../crates/darkly/src/version.rs) (`from_archive`) completes it
   to `v0.9.0-0-g<sha>`. The date is for the metainfo renderer, which adds a
   tarball's own release to the committed history.
2. **`git describe --tags --long`**, in a checkout.
3. **`0.0.0-0-gunknown`**, when neither works.

The file comes first because it names the exact commit the tree came from,
while `git describe` run inside an unpacked tarball or a crates.io build
directory describes whatever repository encloses it: a git-tracked home, or a
Debian `gbp` packaging repository whose tags are `upstream/X.Y.Z` and
`debian/X.Y.Z-N`.

`cargo package` also ships `.cargo_vcs_info.json`, which carries the full SHA
but no tag. It is deliberately not read: it cannot name the version, and one
mechanism for both the tarball and the crate is simpler than two.

## Rules

- **Never read `env!("CARGO_PKG_VERSION")`** or a `package.json` version. Read
  `darkly::VERSION`, or the bridge's `version()` on the frontend. A test fails
  if the crate reverts to it.
- **`desktop/package.json`'s version is written by the release build, not by
  hand.** Electron's packager stamps installer metadata from it and reads no
  other source, so the release build overwrites it from the same describe, builds, and
  restores the file. Packaging the desktop host directly, without that script,
  therefore produces installers labelled `0.0.0`: that is the tell, not a bug to
  work around by typing a real-looking number into the file. It sat at `0.6.0`
  for several releases after a stamped value was committed by accident, which
  made unstamped artifacts indistinguishable from a genuine 0.6.0 build.
  `package-lock.json` carries the same placeholder; npm does not check the root
  version when installing, so the two only need to agree for the reader's sake.
- **Never re-derive the string.** No second `git describe` call.
- **Any CI job that builds or archives needs `fetch-depth: 0`.** A shallow
  checkout has no tags, so the build silently ships `0.0.0-0-gunknown`, and
  `git archive` leaves the describe field of `version.txt` empty.
- **`--long` and no `--always` are deliberate.** The height and SHA are always
  present, so two builds off the same tag never render alike; and a tagless
  checkout throws through to the fallback instead of degrading to a bare SHA.
- `darkly-macros` publishes before `darkly` and at the same version: `darkly`
  pins it by path, so they move in lockstep.

Save-file compatibility is a separate axis: `container_version` and `requires`
in [`format/manifest.rs`](../crates/darkly/src/format/manifest.rs), under [No
Migrations](../CONTRIBUTING.md#no-migrations--no-backwards-compatibility-pre-release).
The app version rides along in a saved file only as a breadcrumb.
