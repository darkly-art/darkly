# Build and install Darkly: the wasm bridge, the frontend, and the Electron
# host, then an install tree for a system Electron. The one recipe every
# channel calls; packaging/README.md documents the targets, the variables and
# the install layout for packagers.
#
#   make tools deps     # once: the wasm-bindgen CLI, then node_modules
#   make                # build the desktop app
#   make install DESTDIR="$pkgdir" PREFIX=/usr ELECTRON=electron44
#
# Offline except `tools` and `deps`. Every variable below can be overridden on
# the command line.

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
ELECTRON     ?= electron

APP_ID       := art.darkly.Darkly
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
.PHONY: app wasm frontend desktop install install-app install-data tools deps clean

app: desktop

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

install: install-app install-data

# The Electron app directory (its package.json names `main`), and a launcher
# that runs it on the system Electron.
install-app:
	@test -f desktop/dist/main.js -a -f desktop/resources/app/index.html \
	  || { echo "nothing to install: run 'make app' first" >&2; exit 1; }
	install -d $(DESTDIR)$(LIBDIR)/darkly/dist $(DESTDIR)$(LIBDIR)/darkly/resources $(DESTDIR)$(BINDIR)
	install -m644 desktop/package.json $(DESTDIR)$(LIBDIR)/darkly/
	cp -R desktop/dist/. $(DESTDIR)$(LIBDIR)/darkly/dist/
	rm -rf $(DESTDIR)$(LIBDIR)/darkly/resources/app
	cp -R desktop/resources/app $(DESTDIR)$(LIBDIR)/darkly/resources/
	sed -e 's|@ELECTRON@|$(ELECTRON)|g' -e 's|@LIBDIR@|$(LIBDIR)|g' packaging/darkly.in \
	  > $(DESTDIR)$(BINDIR)/darkly
	chmod 755 $(DESTDIR)$(BINDIR)/darkly

# Desktop integration, shared with channels that bundle their own Electron.
# The metainfo's <releases> block is refilled from the tags on the way in.
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

# The one target that uses the network. Packagers substitute their own
# vendoring (Flathub's generated sources, a PKGBUILD's prepare()).
deps:
	cd frontend && npm ci
	cd desktop && npm ci

clean:
	rm -rf $(PKG) frontend/dist desktop/dist desktop/resources/app
