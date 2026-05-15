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
```

Before opening a PR, please run the full check suite from [AGENTS.md](AGENTS.md) (fmt, clippy, tests, wasm build, frontend build). Each new feature should have a test; each bug fix should have a regression test (written first, confirmed failing against the unfixed code).

## Questions

Open an issue, or contact <info@darkly.art>.
