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
| `flathub/` | `scripts/flathub.sh`: the Flathub manifest template and the README shipped with its output |

The icons keep the plain `icon` stem rather than the app ID. Electron's packager
derives the platform icon by stripping the extension and appending its own, and
`path.extname('art.darkly.Darkly')` is `.Darkly`, so an app-ID stem makes it look
for `art.darkly.icns`, fail to find it, warn rather than error, and ship macOS
and Windows with no icon at all. Consumers that need the app-ID filename rename
on install, which `make install-data` does.

## What does not belong here

Anything specific to a single packaging target. The Flathub launcher, which
wraps the binary in `zypak-wrapper`, is the worked example: zypak exists only
inside `org.electronjs.Electron2.BaseApp`, so the launcher is meaningless to
every other channel and lives inline in the Flathub manifest template, not
among the shared files. The same rule applies to any future snapcraft or
winget file: shared facts here, per-channel recipes with their channel. The
Flathub template is the one recipe kept in this repository (under `flathub/`)
rather than with its channel, because it is coupled to `Cargo.toml`,
`Cargo.lock`, both npm lockfiles and the Makefile, and here is the only place
an upstream change can be checked against it.

## Building and installing

The root `Makefile` is the whole recipe; a channel's own recipe only fetches
dependencies and calls it. Every channel ships the Electron bundle
`electron-forge package` produces, with the exact Electron
`desktop/package-lock.json` resolves, because that is the Chromium the app's
WebGPU pipeline is tested on. A distro's own Electron is a different Chromium
build on a different schedule, so installing onto one is not supported.

**Build dependencies:** Rust with the `wasm32-unknown-unknown` target, the
`wasm-bindgen` CLI at exactly the version `Cargo.lock` resolves for the
`wasm-bindgen` crate, binaryen's `wasm-opt`, Node.js 22 or newer, npm, and
`make`. The pin lives in the root `Cargo.toml` (`wasm-bindgen = "=0.2.114"`);
`Cargo.lock` follows it, so bump it there. A mismatched CLI refuses with both
versions in the message. fontconfig headers are needed only for the native
test suite and the docs tools, not for `make app`.

**Runtime dependencies:** a Vulkan driver for WebGPU, and the system libraries
Electron's Chromium links (gtk3, nss, libpulse and the rest; a distro's own
Electron package lists the same set, e.g. `pacman -Qi electron44`).

| Target | Does |
| --- | --- |
| `app` (default) | `wasm`, `frontend`, `desktop`, `bundle` in order: the wasm bridge, the frontend in app mode, the frontend staged into `desktop/resources/app` and the Electron host compiled, then `electron-forge package` into `desktop/out/Darkly-linux-<arch>/` |
| `wasm` | `cargo build` of `darkly-wasm`, `wasm-bindgen` into `frontend/wasm/pkg`, and `wasm-opt -O` in the release profile |
| `install` | `install-app` and `install-data` |
| `install-app` | the bundle under `$(LIBDIR)/darkly`, `chrome-sandbox` setuid, and a relative symlink at `$(BINDIR)/darkly` |
| `install-data` | the desktop entry, the metainfo with its release history rendered from the tags, the icon, the license |
| `tools` | `rustup target add wasm32-unknown-unknown` and `cargo install` of the pinned `wasm-bindgen` CLI. For contributors: a packager on distro Rust already has the target |
| `deps` | `npm ci` in `frontend/` and `desktop/` |
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
| `DARKLY_ELECTRON_ZIP_DIR` (environment) | none; a directory holding `electron-v<version>-linux-<arch>.zip`, read by `desktop/forge.config.js` |

`tools`, `deps` and `bundle` use the network; everything else is offline.
`bundle` fetches the Electron zip (into `~/.cache/electron`) unless
`DARKLY_ELECTRON_ZIP_DIR` names a directory that already holds it, so an
offline build lists that zip as a source and points the variable at it.
`make app` does not fetch `node_modules`: every channel has its own offline
story, so it assumes they are present and fails with npm's message when they
are not. `make install` builds nothing; run `make app` first.

**Install layout:**

```
$(BINDIR)/darkly                        -> ../lib/darkly/darkly (relative)
$(LIBDIR)/darkly/darkly                 the bundled Electron, renamed
$(LIBDIR)/darkly/chrome-sandbox         mode 4755
$(LIBDIR)/darkly/resources/app.asar     the Electron host
$(LIBDIR)/darkly/resources/app/         the frontend
$(DATADIR)/applications/art.darkly.Darkly.desktop
$(DATADIR)/metainfo/art.darkly.Darkly.metainfo.xml
$(DATADIR)/icons/hicolor/512x512/apps/art.darkly.Darkly.png
$(LICENSEDIR)/LICENSE
```

`chrome-sandbox` is setuid root, as forge's deb maker and distro Electron
packages install it. Chromium uses it only where unprivileged user namespaces
are unavailable; elsewhere the namespace sandbox is used and the bit is inert.
The symlink is relative, so a `DESTDIR` staging tree runs in place.

**A distro recipe.** For the AUR, in full:

```bash
makedepends=(rust wasm-bindgen binaryen nodejs npm make)
depends=(gtk3 nss libpulse vulkan-icd-loader)
prepare() { cd darkly-$pkgver; (cd frontend && npm ci --ignore-scripts); (cd desktop && npm ci --ignore-scripts); }
build()   { cd darkly-$pkgver; make app; }
package() { cd darkly-$pkgver; make install DESTDIR="$pkgdir" PREFIX=/usr; }
```

`--ignore-scripts` in `desktop/` skips the `electron` package's postinstall
download, which the build never uses: forge reads the Electron version from
`package.json` and unpacks its own zip. CI runs plain `make deps`, which keeps
the download; either works. npm 12 also refuses git dependencies by
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
vendored as a pinned source list. The Flathub repository
(`flathub/art.darkly.Darkly`) holds generated output only: `scripts/flathub.sh`
fills `flathub/art.darkly.Darkly.yaml.in` from the commit at HEAD (the commit,
`rust-version`, the `wasm-bindgen` pin, `Cargo.lock` and the two npm lockfiles)
and the release pipeline pushes the result on every tag
([`docs/versioning.md`](../docs/versioning.md)). CI runs the script and the
Flathub linter on every PR and builds the manifest on the release PR, so an
upstream change that would break Flathub cannot be tagged.

The manifest runs `make app` and `make install PREFIX=/app LIBDIR=/app
LICENSEDIR=/app/share/licenses/art.darkly.Darkly`, then replaces the
`bin/darkly` symlink with its zypak launcher and removes `chrome-sandbox`,
since zypak bridges Chromium's sandbox to the flatpak one. `rust-version` in
`Cargo.toml` is exact (`X.Y.Z`) because it names the toolchain Flathub
downloads; cargo enforces it as a floor, so a build on an older rustc says so.

The summary, description and desktop-entry fields in the two text files are
generated from `crates/darkly/product.yaml`; `cargo sync-docs` refills them.
The metainfo's `<releases>` block is generated from the `v*` tags, never edited
by hand. The committed copy is refreshed after each release
([`docs/versioning.md`](../docs/versioning.md)), so it lags a tag until that
commit lands; `make install-data` therefore refills it at build time from the
tags reachable from the checkout (`scripts/metainfo-releases.sh`), and installs
the result.

The tags are the release record, so a channel that builds from a clone must
fetch them. With no tags at all (a tarball) the committed block is kept, plus
the tarball's own release.

Validate changes to the two text files in this directory the way CI does:

```bash
make install-data DESTDIR=/tmp/darkly-data PREFIX=/usr
appstreamcli validate --no-net /tmp/darkly-data/usr/share/metainfo/art.darkly.Darkly.metainfo.xml
desktop-file-validate /tmp/darkly-data/usr/share/applications/art.darkly.Darkly.desktop
```

To build the generated manifest the way Flathub does, with `org.flatpak.Builder`
installed from Flathub (it needs the multi-gigabyte Freedesktop SDK and the
Electron base app):

```bash
scripts/flathub.sh /tmp/flathub && cd /tmp/flathub
flatpak run org.flatpak.Builder --force-clean --sandbox --user --install-deps-from=flathub \
  --ccache --compose-url-policy=full --mirror-screenshots-url=https://dl.flathub.org/media/ \
  --repo=repo --install builddir art.darkly.Darkly.yaml
flatpak run --command=flatpak-builder-lint org.flatpak.Builder manifest art.darkly.Darkly.yaml
flatpak run art.darkly.Darkly
```
