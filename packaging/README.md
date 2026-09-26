# Packaging resources

Desktop-integration metadata and artwork shared by every channel Darkly ships
through. One home each, so a description or an icon is never restated per
packaging target.

| File | Consumed by |
| --- | --- |
| `art.darkly.Darkly.desktop` | Flathub manifest; the deb and AppImage makers |
| `art.darkly.Darkly.metainfo.xml` | AppStream, so GNOME Software, KDE Discover and Bazaar |
| `icon.png` | Flathub (installed as `art.darkly.Darkly.png`), deb, AppImage |
| `icon.icns` | macOS `.dmg` and `.zip` |
| `icon.ico` | Windows installer |
| `app.json` | `desktop/forge.config.js` (generated from `product.yaml`) |
| `darkly.in` | `make install-app`, which renders it to the `darkly` launcher |

The icons keep the plain `icon` stem rather than the app ID. Electron's packager
derives the platform icon by stripping the extension and appending its own, and
`path.extname('art.darkly.Darkly')` is `.Darkly`, so an app-ID stem makes it look
for `art.darkly.icns`, fail to find it, warn rather than error, and ship macOS
and Windows with no icon at all. Consumers that need the app-ID filename rename
on install, which `make install-data` does.

## What does not belong here

Anything specific to a single packaging target. `darkly.in` is shared because
every distro channel runs Darkly on a system Electron the same way. The Flathub
launcher, which
wraps the binary in `zypak-wrapper`, is the worked example: zypak exists only
inside `org.electronjs.Electron2.BaseApp`, so the launcher is meaningless to
every other channel and belongs with the Flathub manifest rather than here,
whether it is carried as its own file or declared inline in that manifest. The
same rule applies to any future snapcraft or winget file: shared facts here,
per-channel recipes with their channel.

## Building and installing

The root `Makefile` is the whole recipe; a channel's own recipe only fetches
dependencies and calls it.

**Build dependencies:** Rust with the `wasm32-unknown-unknown` target, the
`wasm-bindgen` CLI at exactly the version `Cargo.lock` resolves for the
`wasm-bindgen` crate, binaryen's `wasm-opt`, Node.js 22 or newer, npm, and
`make`. The pin lives in the root `Cargo.toml` (`wasm-bindgen = "=0.2.114"`);
`Cargo.lock` follows it, so bump it there. A mismatched CLI refuses with both
versions in the message. fontconfig headers are needed only for the native
test suite and the docs tools, not for `make app`.

**Runtime dependencies:** Electron (44 is what `desktop/package-lock.json`
resolves and what CI runs), and a Vulkan driver for WebGPU.

| Target | Does |
| --- | --- |
| `app` (default) | `wasm`, `frontend`, `desktop` in order: the wasm bridge, the frontend in app mode, then the frontend staged into `desktop/resources/app` and the Electron host compiled |
| `wasm` | `cargo build` of `darkly-wasm`, `wasm-bindgen` into `frontend/wasm/pkg`, and `wasm-opt -O` in the release profile |
| `install` | `install-app` and `install-data` |
| `install-app` | the app tree under `$(LIBDIR)/darkly` and the launcher at `$(BINDIR)/darkly` |
| `install-data` | the desktop entry, the metainfo with its release history rendered from the tags, the icon, the license |
| `tools` | `rustup target add wasm32-unknown-unknown` and `cargo install` of the pinned `wasm-bindgen` CLI. For contributors: a packager on distro Rust already has the target |
| `deps` | `npm ci` in `frontend/` and `desktop/`. The only target that uses the network |
| `clean` | removes the build outputs |

| Variable | Default |
| --- | --- |
| `PROFILE` | `release`; `dev` skips `wasm-opt` |
| `FEATURES` | none; cargo features for `darkly-wasm` |
| `WASM_BINDGEN`, `WASM_OPT` | `wasm-bindgen`, `wasm-opt` |
| `DESTDIR` | none |
| `PREFIX` | `/usr/local` |
| `BINDIR`, `LIBDIR`, `DATADIR` | `$(PREFIX)/bin`, `$(PREFIX)/lib`, `$(PREFIX)/share` |
| `LICENSEDIR` | `$(DATADIR)/licenses/darkly` |
| `ELECTRON` | `electron`; the command the launcher runs, e.g. `electron44` |

`make app` does not fetch `node_modules`: every channel has its own offline
story, so it assumes they are present and fails with npm's message when they
are not. `make install` builds nothing; run `make app` first.

**Install layout:**

```
$(BINDIR)/darkly                        exec $(ELECTRON) $(LIBDIR)/darkly "$@"
$(LIBDIR)/darkly/package.json           Electron reads `main` from it
$(LIBDIR)/darkly/dist/                  the compiled Electron host
$(LIBDIR)/darkly/resources/app/         the frontend
$(DATADIR)/applications/art.darkly.Darkly.desktop
$(DATADIR)/metainfo/art.darkly.Darkly.metainfo.xml
$(DATADIR)/icons/hicolor/512x512/apps/art.darkly.Darkly.png
$(LICENSEDIR)/LICENSE
```

The installed tree needs no `node_modules`. The launcher bakes in `$(LIBDIR)`,
so it runs only once installed there; stage with `DESTDIR`, not `PREFIX`.
`electron-flags.conf` handling is left to the distro's own Electron launcher.

**A distro recipe.** For the AUR, in full:

```bash
makedepends=(rust wasm-bindgen binaryen nodejs npm make)
depends=(electron44)
prepare() { cd darkly-$pkgver; (cd frontend && npm ci --ignore-scripts); (cd desktop && npm ci --ignore-scripts); }
build()   { cd darkly-$pkgver; make app; }
package() { cd darkly-$pkgver; make install DESTDIR="$pkgdir" PREFIX=/usr ELECTRON=electron44; }
```

`--ignore-scripts` in `desktop/` skips Electron's binary download, which a
system-Electron install does not want. npm 12 also refuses git dependencies by
default, and `desktop/package-lock.json` has one (`@electron/node-gyp`, through
electron-forge), so on npm 12 add `--allow-git=all` there. When the distro's
`wasm-bindgen` is not the pinned version (Arch ships a newer one), install the
pinned CLI with `cargo install wasm-bindgen-cli --locked --version <pin>`, or
`make tools`, and put it first on `PATH`.

**The version.** A release tarball has no `.git`, so the build reads its version
from `crates/darkly/version.txt`, which `git archive` fills in; see
[`docs/versioning.md`](../docs/versioning.md). A tarball's metainfo carries the
release history as committed at the tag, plus the tarball's own release, which
`install-data` adds from the same file.

**Tests.** `cargo test --workspace --exclude darkly-wasm --features
darkly/testing -- --test-threads=1`, with a Vulkan device (a real one, or Mesa's
lavapipe with `WGPU_BACKEND=vulkan`) and the fontconfig headers.
`--test-threads=1` is required: GPU tests share one device. The suite holds in a
tarball too: the version tests accept either the placeholders or a filled-in
`version.txt`.

## Flathub

Darkly is published to Flathub as `art.darkly.Darkly`. Flathub builds from
source on its own infrastructure with no network access, so every dependency is
vendored as a pinned source list rather than fetched at build time, and the
manifest lives in the `flathub/art.darkly.Darkly` repository rather than here.

The summary, description and desktop-entry fields in the two text files are
generated from `crates/darkly/product.yaml`; `cargo sync-docs` refills them.
The metainfo's `<releases>` block is generated from the `v*` tags, never edited
by hand. The committed copy is refreshed after each release
([`docs/versioning.md`](../docs/versioning.md)), so it lags a tag until that
commit lands; `make install-data` therefore refills it at build time from the
tags reachable from the checkout (`scripts/metainfo-releases.sh`), and installs
the result. Flathub bundles its own Electron, so its manifest uses
`install-data` alone:

```bash
make install-data DESTDIR=/ PREFIX=/app LICENSEDIR=/app/share/licenses/art.darkly.Darkly
```

The tags are the release record, so a channel that builds from a clone must
fetch them. With no tags at all (a tarball) the committed block is kept, plus
the tarball's own release.

Validate changes to the two text files in this directory the way CI does:

```bash
make install-data DESTDIR=/tmp/darkly-data PREFIX=/usr
appstreamcli validate --no-net /tmp/darkly-data/usr/share/metainfo/art.darkly.Darkly.metainfo.xml
desktop-file-validate /tmp/darkly-data/usr/share/applications/art.darkly.Darkly.desktop
```

The full manifest build and lint recipe is documented alongside the manifest in
the Flathub repository, since it needs the multi-gigabyte Freedesktop SDK and
the Electron base app.
