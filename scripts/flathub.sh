#!/usr/bin/env bash
# Generate the Flathub packaging for the commit at HEAD: the manifest, from
# packaging/flathub/art.darkly.Darkly.yaml.in, and the vendored source lists
# flatpak-builder needs to build with no network.
#
#   scripts/flathub.sh <outdir>      # <outdir> must be empty or absent
#
# Every pin comes from the tree, so the output is a function of the commit:
# the commit itself; `rust-version` in Cargo.toml (exact, X.Y.Z) for the Rust
# toolchain; the wasm-bindgen pin in Cargo.lock for the CLI, through
# `make -s wasm-bindgen-version`; Cargo.lock and the two npm lockfiles through
# flatpak-builder-tools' generators. Hashes for the prebuilt toolchain tarballs
# come from their publishers' sha256 sidecars. Everything is read from
# `git archive HEAD`, never from the working tree, so ignored files and
# uncommitted edits cannot leak into the lists.
#
# Run it from inside the checkout to generate for. Needs git, curl, make,
# python3 with venv, and the network. CI runs it on every PR and lints the
# result; the release pipeline runs it at the tag and pushes the output to
# flathub/art.darkly.Darkly (docs/versioning.md). CURL,
# FLATPAK_CARGO_GENERATOR and FLATPAK_NODE_GENERATOR override the commands,
# which the test in crates/darkly/tests/release_scripts.rs uses.
set -euo pipefail

# flatpak/flatpak-builder-tools has no releases. This is master on 2026-09-21;
# anything past 1fc3219 ("node: Drop armv7l for electron 44+", PR #551) works.
FLATPAK_BUILDER_TOOLS_REV=41c20aa10819cdb2a4f3ca171758a96d1955c018

die() { echo "flathub: $*" >&2; exit 1; }

out="${1:?usage: $0 <outdir>}"
mkdir -p "$out"
[ -z "$(ls -A "$out")" ] || die "$out is not empty"
out=$(cd "$out" && pwd)

repo=$(git rev-parse --show-toplevel)
commit=$(git rev-parse HEAD)

curl=${CURL:-curl}
# The first field of a sha256sum-style sidecar, checked to be a sha256.
sidecar() {
  local h
  h=$($curl -fsSL "$1" | awk '{print $1; exit}')
  [[ "$h" =~ ^[0-9a-f]{64}$ ]] || die "no sha256 at $1"
  echo "$h"
}

# The generators: overridden, or installed once into a cached venv. The cargo
# generator is a single file with no entry point of its own.
if [ -z "${FLATPAK_NODE_GENERATOR:-}" ] || [ -z "${FLATPAK_CARGO_GENERATOR:-}" ]; then
  tools="${XDG_CACHE_HOME:-$HOME/.cache}/darkly/flatpak-builder-tools-$FLATPAK_BUILDER_TOOLS_REV"
  if [ ! -x "$tools/bin/flatpak-node-generator" ]; then
    rm -rf "$tools"
    python3 -m venv "$tools"
    "$tools/bin/pip" install --quiet \
      "git+https://github.com/flatpak/flatpak-builder-tools@$FLATPAK_BUILDER_TOOLS_REV#subdirectory=node" \
      aiohttp tomlkit pyyaml
    $curl -fsSL "https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/$FLATPAK_BUILDER_TOOLS_REV/cargo/flatpak-cargo-generator.py" \
      > "$tools/bin/flatpak-cargo-generator.py"
  fi
  : "${FLATPAK_NODE_GENERATOR:=$tools/bin/flatpak-node-generator}"
  : "${FLATPAK_CARGO_GENERATOR:=$tools/bin/python $tools/bin/flatpak-cargo-generator.py}"
fi

tree=$(mktemp -d)
trap 'rm -rf "$tree"' EXIT
git -C "$repo" archive HEAD | tar -x -C "$tree"
cd "$tree"

template=packaging/flathub/art.darkly.Darkly.yaml.in
[ -f "$template" ] || die "no $template at $commit"

rust=$(sed -n 's/^rust-version = "\(.*\)"$/\1/p' Cargo.toml | head -1)
[[ "$rust" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] \
  || die "rust-version in Cargo.toml must be exact (X.Y.Z), got '$rust'"
wbg=$(make -s wasm-bindgen-version)
[ -n "$wbg" ] || die "make wasm-bindgen-version printed nothing"

rust_dist=https://static.rust-lang.org/dist
wbg_dist=https://github.com/wasm-bindgen/wasm-bindgen/releases/download/$wbg
rust_x86_64=$(sidecar "$rust_dist/rust-$rust-x86_64-unknown-linux-gnu.tar.xz.sha256")
rust_aarch64=$(sidecar "$rust_dist/rust-$rust-aarch64-unknown-linux-gnu.tar.xz.sha256")
rust_wasm=$(sidecar "$rust_dist/rust-std-$rust-wasm32-unknown-unknown.tar.xz.sha256")
wbg_x86_64=$(sidecar "$wbg_dist/wasm-bindgen-$wbg-x86_64-unknown-linux-musl.tar.gz.sha256sum")
wbg_aarch64=$(sidecar "$wbg_dist/wasm-bindgen-$wbg-aarch64-unknown-linux-musl.tar.gz.sha256sum")

# shellcheck disable=SC2086  # the overrides may carry an interpreter
$FLATPAK_CARGO_GENERATOR Cargo.lock -o "$out/cargo-sources.json"
# `-r` walks the tree for every package-lock.json: frontend/ and desktop/.
$FLATPAK_NODE_GENERATOR -r -R package-lock.json -o "$out/generated-sources.json" npm package-lock.json

sed -e "s|@COMMIT@|$commit|g" \
    -e "s|@RUST_VERSION@|$rust|g" \
    -e "s|@RUST_X86_64_SHA256@|$rust_x86_64|g" \
    -e "s|@RUST_AARCH64_SHA256@|$rust_aarch64|g" \
    -e "s|@RUST_WASM_SHA256@|$rust_wasm|g" \
    -e "s|@WASM_BINDGEN_VERSION@|$wbg|g" \
    -e "s|@WASM_BINDGEN_X86_64_SHA256@|$wbg_x86_64|g" \
    -e "s|@WASM_BINDGEN_AARCH64_SHA256@|$wbg_aarch64|g" \
    "$template" > "$out/art.darkly.Darkly.yaml"
! grep -qE '@[A-Z0-9_]+@' "$out/art.darkly.Darkly.yaml" \
  || die "unfilled placeholder: $(grep -oE '@[A-Z0-9_]+@' "$out/art.darkly.Darkly.yaml" | sort -u | tr '\n' ' ')"
cp packaging/flathub/README.md "$out/README.md"

echo "flathub: $out <- $commit (rust $rust, wasm-bindgen $wbg)"
