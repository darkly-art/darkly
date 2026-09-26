//! The grammar of Darkly's version string, shared by `build.rs` (which bakes
//! [`crate::VERSION`]) and the library's tests (which hold the baked value to
//! it). Std-only, because `build.rs` includes this file by path.
//!
//! The version is `git describe --tags --long` (`v0.3.0-1-gf0c3ea9`). A tree
//! with no `.git` (a release tarball, the crates.io package) reads it instead
//! from `version.txt`, which `git archive` fills in through `export-subst`; see
//! `docs/versioning.md`.

/// The version of a build that has neither a filled-in `version.txt` nor a
/// describable repository. Parses as a describe string, tag `0.0.0`.
pub const FALLBACK: &str = "0.0.0-0-gunknown";

/// Split a `git describe --tags --long` string into its tag, commit height
/// and `g`-prefixed short SHA, or `None` for anything not of that shape.
/// `v0.3.0-1-gf0c3ea9` gives `("v0.3.0", 1, "gf0c3ea9")`; a tag may itself
/// contain dashes, so the split runs from the right.
pub fn describe_parts(version: &str) -> Option<(&str, u32, &str)> {
    let (rest, sha) = version.rsplit_once('-')?;
    let (tag, height) = rest.rsplit_once('-')?;
    if tag.is_empty() || !sha.starts_with('g') || sha.len() < 2 {
        return None;
    }
    Some((tag, height.parse().ok()?, sha))
}

/// The version recorded in `version.txt`, or `None` when the file still holds
/// its placeholders (a checkout) or names no tag (an archive of an untagged
/// or shallow history).
///
/// The last line is `<describe> <sha> <date>`. git's `%(describe)` has no
/// `--long`, so at a tag it yields the bare tag and the SHA field supplies the
/// rest: `v0.9.0 abc1234` gives `v0.9.0-0-gabc1234`. Past a tag it is already
/// the long shape and is returned as is. The date is the release renderer's,
/// not this function's.
pub fn from_archive(text: &str) -> Option<String> {
    let line = text
        .lines()
        .rfind(|l| !l.trim().is_empty() && !l.starts_with('#'))?;
    if line.contains("$Format") {
        return None;
    }
    let mut fields = line.split(' ');
    let describe = fields.next()?;
    let sha = fields.next()?;
    if describe.is_empty() || sha.is_empty() {
        return None;
    }
    if describe_parts(describe).is_some() {
        return Some(describe.to_string());
    }
    Some(format!("{describe}-0-g{sha}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::process::Command;

    const CHECKED_IN: &str = include_str!("../version.txt");

    #[test]
    fn describe_parts_reads_tag_height_and_sha() {
        assert_eq!(
            describe_parts("v0.8.0-3-gabc1234"),
            Some(("v0.8.0", 3, "gabc1234"))
        );
        assert_eq!(describe_parts(FALLBACK), Some(("0.0.0", 0, "gunknown")));
        assert_eq!(
            describe_parts("my-tag-2-gdeadbee"),
            Some(("my-tag", 2, "gdeadbee"))
        );
        assert_eq!(describe_parts("0.1.0"), None);
        assert_eq!(describe_parts("v0.8.0-x-gabc"), None);
        assert_eq!(describe_parts("v0.8.0-3-abc"), None);
    }

    #[test]
    fn from_archive_reads_a_substituted_file() {
        let file = |line: &str| format!("# comment\n{line}\n");
        assert_eq!(
            from_archive(&file("v0.9.0 abc1234 2026-09-25")),
            Some("v0.9.0-0-gabc1234".into())
        );
        assert_eq!(
            from_archive(&file("v0.9.0-3-gabc1234 abc1234 2026-09-25")),
            Some("v0.9.0-3-gabc1234".into())
        );
    }

    #[test]
    fn from_archive_rejects_placeholders_and_untagged_archives() {
        assert_eq!(
            from_archive("# c\n$Format:%(describe:tags)$ $Format:%h$ $Format:%cs$\n"),
            None
        );
        assert_eq!(from_archive("# c\n abc1234 2026-09-25\n"), None);
        assert_eq!(from_archive("# only a comment\n"), None);
        assert_eq!(from_archive(""), None);
    }

    /// Placeholders in a checkout; filled in in a release tarball, where a
    /// packager runs this same suite.
    #[test]
    fn checked_in_file_is_placeholders_or_a_version() {
        let placeholders =
            CHECKED_IN.contains("$Format:%(describe:tags)$ $Format:%h$ $Format:%cs$");
        assert!(
            placeholders || from_archive(CHECKED_IN).is_some(),
            "version.txt is neither the placeholders nor a readable version:\n{CHECKED_IN}"
        );
    }

    /// `git archive` substitutes only files marked `export-subst`. Skipped
    /// outside this repository: a tarball has no `.git`, or sits inside
    /// someone else's (a packaging repository, a git-tracked home).
    #[test]
    fn version_file_is_marked_export_subst() {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let git = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(manifest)
                .output()
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
        };
        let Some(toplevel) = git(&["rev-parse", "--show-toplevel"]) else {
            return;
        };
        let ours = manifest.join("../..").canonicalize().unwrap();
        if Path::new(&toplevel).canonicalize().ok() != Some(ours) {
            return;
        }
        let Some(attr) = git(&["check-attr", "export-subst", "--", "version.txt"]) else {
            return;
        };
        assert_eq!(attr, "version.txt: export-subst: set");
    }
}
