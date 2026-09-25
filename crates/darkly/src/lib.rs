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
/// build.rs as `DARKLY_VERSION`. The single crate-side home for the version;
/// consumers read this, never `env!("CARGO_PKG_VERSION")` (which is the stale
/// hardcoded `Cargo.toml` value). See `frontend/src/version.ts` for the
/// frontend twin that derives its display version from the same git tags.
pub const VERSION: &str = env!("DARKLY_VERSION");

#[cfg(test)]
mod version_tests {
    use super::VERSION;
    use std::process::Command;

    /// Split a `git describe --tags --long` string into its tag, commit height
    /// and `g`-prefixed short SHA, or `None` for anything not of that shape. The
    /// shape the version tests hold `VERSION` to; `build.rs` is where it is
    /// written. `v0.3.0-1-gf0c3ea9` gives `("v0.3.0", 1, "gf0c3ea9")`;
    /// a tag may itself contain dashes, so the split runs from the right.
    fn describe_parts(version: &str) -> Option<(&str, u32, &str)> {
        let (rest, sha) = version.rsplit_once('-')?;
        let (tag, height) = rest.rsplit_once('-')?;
        if tag.is_empty() || !sha.starts_with('g') || sha.len() < 2 {
            return None;
        }
        Some((tag, height.parse().ok()?, sha))
    }

    /// Does `s` have the `git describe --tags --long` shape `<tag>-<n>-g<sha>`?
    /// True for `v0.3.0-1-gf0c3ea9` and the `0.0.0-0-gunknown` fallback, but
    /// false for a bare semver like `0.1.0`.
    fn is_describe_shape(s: &str) -> bool {
        describe_parts(s).is_some()
    }

    #[test]
    fn describe_parts_reads_tag_height_and_sha() {
        assert_eq!(
            describe_parts("v0.8.0-3-gabc1234"),
            Some(("v0.8.0", 3, "gabc1234"))
        );
        assert_eq!(
            describe_parts("0.0.0-0-gunknown"),
            Some(("0.0.0", 0, "gunknown"))
        );
        assert_eq!(
            describe_parts("my-tag-2-gdeadbee"),
            Some(("my-tag", 2, "gdeadbee"))
        );
        assert_eq!(describe_parts("0.1.0"), None);
        assert_eq!(describe_parts("v0.8.0-x-gabc"), None);
        assert_eq!(describe_parts("v0.8.0-3-abc"), None);
    }

    /// Feature test: the baked version really is the live `git describe` value
    /// when git + tags are available. Only asserts equality when the command
    /// succeeds, so it actually exercises the pipeline rather than passing
    /// vacuously on the fallback in a shallow/git-less CI checkout.
    ///
    /// (Build-time vs. test-time describe could differ if the repo mutates
    /// mid-run, though that's negligible within a single `cargo test`.)
    #[test]
    fn version_matches_live_git_describe() {
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
                VERSION, "0.0.0-0-gunknown",
                "no git/tags → fallback expected"
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
            is_describe_shape(VERSION),
            "unexpected version shape: {VERSION}"
        );
    }
}
