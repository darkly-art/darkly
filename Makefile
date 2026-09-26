# Build and install Darkly: the wasm bridge, the frontend, the Electron host,
# and the forge bundle that ships with the exact Electron the app is tested
# on. The one recipe every channel calls; packaging/README.md documents the
# targets, the variables and the install layout for packagers.
#
#   make tools deps     # once: the wasm-bindgen CLI, then node_modules
#   make                # build the desktop app bundle
#   make install DESTDIR="$pkgdir" PREFIX=/usr
#
# Offline except `tools`, `deps`, and `bundle`, which fetches the Electron zip
# unless DARKLY_ELECTRON_ZIP_DIR names a directory that already holds it (see
# desktop/forge.config.js). Every variable below can be overridden on the
# command line.

PROFILE      ?= release
FEATURES     ?=
WASM_BINDGEN ?= wasm-bindgen
WASM_OPT     ?= wasm-opt

DESTDIR      ?=
PREFIX       ?= /usr/local
BINDIR       ?= $(PREFIX)/bin
LIBDIR       ?= $(PREFIX)/lib
DATADIR      ?= $(PREFIX)/share
LICENSEDIR   ?= $(DATADIR)/licenses/darkly

# The same id is the app's desktop name in desktop/src/main.ts.
APP_ID       := art.darkly.Darkly
BUNDLE       := desktop/out/Darkly-linux-*
WASM_TARGET  := wasm32-unknown-unknown
PKG          := frontend/wasm/pkg
TARGET_DIR   := $(or $(CARGO_TARGET_DIR),target)

# Cargo writes the `dev` profile to `debug/`. wasm-bindgen's `--debug` keeps
# the JS glue's assertions in a dev build; the release build is optimized in
# place with wasm-opt instead.
ifeq ($(PROFILE),release)
  PROFILE_DIR   := release
  BINDGEN_FLAGS :=
else ifeq ($(PROFILE),dev)
  PROFILE_DIR   := debug
  BINDGEN_FLAGS := --debug
else
  $(error PROFILE must be release or dev, not '$(PROFILE)')
endif

# The CLI must match the `wasm-bindgen` crate exactly. The pin lives in the
# root Cargo.toml; this reads the version Cargo.lock resolved from it.
WASM_BINDGEN_VERSION = $(shell sed -n '/^name = "wasm-bindgen"$$/{n;s/^version = "\(.*\)"$$/\1/p;}' Cargo.lock)

# `frontend` and `desktop` are also directory names: without .PHONY, make
# would answer "is up to date" and build nothing.
.PHONY: app wasm frontend desktop bundle install install-app install-data tools deps clean

app: bundle

wasm:
	cargo build -p darkly-wasm --target $(WASM_TARGET) --profile $(PROFILE) $(if $(FEATURES),--features $(FEATURES))
	$(WASM_BINDGEN) --target web $(BINDGEN_FLAGS) --out-dir $(PKG) --out-name darkly_wasm \
	  $(TARGET_DIR)/$(WASM_TARGET)/$(PROFILE_DIR)/darkly_wasm.wasm
ifeq ($(PROFILE),release)
	$(WASM_OPT) -O -o $(PKG)/darkly_wasm_bg.wasm $(PKG)/darkly_wasm_bg.wasm
endif

frontend: wasm
	cd frontend && npx vite build --mode app

desktop: frontend
	rm -rf desktop/resources/app
	mkdir -p desktop/resources/app
	cp -R frontend/dist/. desktop/resources/app/
	cd desktop && npx tsc

# The app with its own Electron: desktop/out/Darkly-linux-<arch>/.
bundle: desktop
	cd desktop && npx electron-forge package

install: install-app install-data

# The bundle under $(LIBDIR)/darkly and a relative symlink to its executable
# in $(BINDIR), the shape forge's deb maker installs. `cp -R` without `-p`
# drops the setuid bit Chromium's sandbox helper needs where unprivileged
# user namespaces are unavailable, so it is set again afterwards; `-p` would
# record the build user's ownership under fakeroot.
install-app:
	@test "$(words $(wildcard $(BUNDLE)))" = 1 \
	  || { echo "expected exactly one $(BUNDLE): run 'make app' first" >&2; exit 1; }
	rm -rf $(DESTDIR)$(LIBDIR)/darkly
	install -d $(DESTDIR)$(LIBDIR)/darkly $(DESTDIR)$(BINDIR)
	cp -R $(wildcard $(BUNDLE))/. $(DESTDIR)$(LIBDIR)/darkly/
	chmod 4755 $(DESTDIR)$(LIBDIR)/darkly/chrome-sandbox
	ln -sfr $(DESTDIR)$(LIBDIR)/darkly/darkly $(DESTDIR)$(BINDIR)/darkly

# Desktop integration. The metainfo's <releases> block is refilled from the
# tags on the way in.
install-data:
	install -Dm644 packaging/$(APP_ID).desktop $(DESTDIR)$(DATADIR)/applications/$(APP_ID).desktop
	install -d $(DESTDIR)$(DATADIR)/metainfo
	scripts/metainfo-releases.sh packaging/$(APP_ID).metainfo.xml \
	  > $(DESTDIR)$(DATADIR)/metainfo/$(APP_ID).metainfo.xml
	chmod 644 $(DESTDIR)$(DATADIR)/metainfo/$(APP_ID).metainfo.xml
	install -Dm644 packaging/icon.png $(DESTDIR)$(DATADIR)/icons/hicolor/512x512/apps/$(APP_ID).png
	install -Dm644 LICENSE $(DESTDIR)$(LICENSEDIR)/LICENSE

# Contributor setup. A packager on distro Rust already has the wasm32 target.
tools:
	rustup target add $(WASM_TARGET)
	@test -n "$(WASM_BINDGEN_VERSION)" || { echo "wasm-bindgen not found in Cargo.lock" >&2; exit 1; }
	cargo install --locked wasm-bindgen-cli --version $(WASM_BINDGEN_VERSION)

# Packagers substitute their own vendoring (Flathub's generated sources, a
# PKGBUILD's prepare()).
deps:
	cd frontend && npm ci
	cd desktop && npm ci

clean:
	rm -rf $(PKG) frontend/dist desktop/dist desktop/resources/app desktop/out
