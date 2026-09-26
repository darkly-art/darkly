pub mod action;
pub mod actions;
pub mod brush;
pub mod catalog;
pub mod clipboard;
pub mod config;
pub mod coord;
/// Fills the marked regions of the repository's own markdown from the
/// registries. Repository tooling that walks a source tree, so it is native-only:
/// a browser has no checkout to sync.
#[cfg(not(target_arch = "wasm32"))]
pub mod docs_md;
/// Renders the documentation preview assets. Performs blocking GPU readbacks,
/// so it lives behind the same gate as `gpu::test_utils`: engine, compositor
/// and WASM-bridge code cannot name it in a production build.
#[cfg(any(test, feature = "testing"))]
pub mod docs_render;
pub mod document;
pub mod engine;
pub mod format;
pub mod gpu;
pub mod layer;
pub mod mask;
pub mod nodegraph;
/// What Darkly says it is, loaded from `product.yaml`. Packaging metadata for
/// the tooling that generates store listings and desktop entries, so it lives
/// behind the same gate as `docs_md`: a browser ships no desktop entry.
#[cfg(not(target_arch = "wasm32"))]
pub mod product;
pub mod sdf;
pub mod text;
pub mod tool;
pub mod tools;
pub mod transform;
pub mod undo;
pub mod units;

/// Darkly's version: the latest git tag plus the commit height since it
/// (`git describe --tags --long`, e.g. `v0.3.0-1-gf0c3ea9`), baked in by
/// build.rs as `DARKLY_VERSION`. The single home for the version; consumers
/// read this, never `env!("CARGO_PKG_VERSION")` (which is the stale hardcoded
/// `Cargo.toml` value). The frontend reads it through the wasm bridge.
pub const VERSION: &str = env!("DARKLY_VERSION");

/// The version grammar `build.rs` bakes [`VERSION`] with. `build.rs` includes
/// the same file by path, so the library compiles it only to test it.
#[cfg(test)]
mod version;

#[cfg(test)]
mod version_tests {
    use super::version::{describe_parts, from_archive, FALLBACK};
    use super::VERSION;
    use std::process::Command;

    /// The baked version is whichever source `build.rs` gives precedence: a
    /// filled-in `version.txt`, else live `git describe`, else the fallback.
    /// Asserting against the source that applies exercises the pipeline in a
    /// checkout, a tarball and a git-less tree alike.
    ///
    /// (Build-time vs. test-time describe could differ if the repo mutates
    /// mid-run, though that's negligible within a single `cargo test`.)
    #[test]
    fn version_matches_its_source() {
        if let Some(archive) = from_archive(include_str!("../version.txt")) {
            assert_eq!(VERSION, archive, "baked version should equal version.txt");
            return;
        }
        let live = Command::new("git")
            .args(["describe", "--tags", "--long"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        match live {
            Some(desc) => assert_eq!(VERSION, desc, "baked version should equal live describe"),
            None => assert_eq!(
                VERSION, FALLBACK,
                "no archive, git or tags: fallback expected"
            ),
        }
    }

    /// Regression guard (NOT the feature test): the version must never silently
    /// revert to the stale `Cargo.toml` semver. The describe form always carries
    /// `-<n>-g<sha>`, so it can never equal a bare semver.
    #[test]
    fn version_is_not_cargo_pkg_version() {
        assert_ne!(VERSION, env!("CARGO_PKG_VERSION"));
        assert!(
            describe_parts(VERSION).is_some(),
            "unexpected version shape: {VERSION}"
        );
    }
}
