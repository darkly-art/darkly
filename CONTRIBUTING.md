# Contributing to Darkly

Thanks for wanting to contribute.

## How to contribute

The development setup, build commands, and project conventions live in [AGENTS.md](AGENTS.md). The short version:

```bash
# Rust core + tests
cargo check --workspace
cargo test --workspace --exclude darkly-wasm -- --test-threads=1

# WASM bridge
(cd frontend/wasm && wasm-pack build --release --target web --out-dir pkg)

# Frontend
(cd frontend && npm install && npm run dev)

# Git hooks — keeps the generated parts of the docs in sync (see AGENTS.md)
scripts/install-hooks.sh
```

Some of the markdown here is generated from Darkly's registries and marked with
`<!-- darkly:… -->` comments. Don't edit inside those regions; edit the
registration the text comes from. See [AGENTS.md](AGENTS.md#generated-markdown).

Before opening a PR, please run the full check suite from [AGENTS.md](AGENTS.md) (fmt, clippy, tests, wasm build, frontend build). Each new feature should have a test; each bug fix should have a regression test (written first, confirmed failing against the unfixed code).

## Questions

Open an issue, or contact <info@darkly.art>.
