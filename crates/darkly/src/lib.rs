pub mod brush;
pub mod clipboard;
pub mod config;
pub mod coord;
pub mod document;
pub mod engine;
pub mod format;
pub mod gpu;
pub mod layer;
pub mod mask;
pub mod nodegraph;
pub mod sdf;
pub mod tile;
pub mod tool;
pub mod tools;
pub mod transform;
pub mod undo;

/// Darkly's version — the latest git tag plus the commit height since it
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

    /// Does `s` have the `git describe --tags --long` shape `<tag>-<n>-g<sha>`?
    /// True for `v0.3.0-1-gf0c3ea9` and the `0.0.0-0-gunknown` fallback, but
    /// false for a bare semver like `0.1.0`.
    fn is_describe_shape(s: &str) -> bool {
        // rsplitn(3) -> [g<sha>, <n>, <tag-possibly-with-dashes>]
        let parts: Vec<&str> = s.rsplitn(3, '-').collect();
        parts.len() == 3
            && parts[0].starts_with('g')
            && parts[0].len() > 1
            && parts[1].parse::<u32>().is_ok()
            && !parts[2].is_empty()
    }

    /// Feature test: the baked version really is the live `git describe` value
    /// when git + tags are available. Only asserts equality when the command
    /// succeeds, so it actually exercises the pipeline rather than passing
    /// vacuously on the fallback in a shallow/git-less CI checkout.
    ///
    /// (Build-time vs. test-time describe could differ if the repo mutates
    /// mid-run — negligible within a single `cargo test`.)
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
