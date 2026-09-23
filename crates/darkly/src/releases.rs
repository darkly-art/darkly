//! Loads [`releases.json`](../releases.json), the release history the store
//! listing and the GitHub release page are rendered from, and the pure halves
//! of the tool that writes it.
//!
//! A release is a range of commits: everything between the previous `v*` tag
//! and the commit being released. The merge subjects in that range name the
//! pull requests it ships, and each PR's title is the user-facing line for
//! that change. `cargo releases --fetch X.Y.Z` (`src/bin/releases.rs`)
//! resolves the range with `git`, reads titles with `gh`, and writes one entry
//! here; it is the only writer. A wrong line is fixed on the PR title on GitHub
//! and re-fetched, never edited in the file.
//!
//! Embedded at compile time like `product.yaml`, so the `docs_md` fragment that
//! renders it stays a pure function of the crate and the sync check runs
//! offline. Everything that decides something (which tag is previous, which
//! subject names a PR, whether a title is store-safe, how an entry sorts) takes
//! plain strings and lives here so it is testable without a repository; the
//! binary only runs the fixed commands and hands their output in.

use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;
use std::sync::OnceLock;

/// The authored snapshot, embedded so a fragment needs no repository root.
const SOURCE: &str = include_str!("../releases.json");

/// The repository every rendered URL points into. One home, inherited from the
/// workspace manifest, so the snapshot never stores a URL that could disagree
/// with it.
const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");

/// Where the snapshot lives, relative to the repository root. The binary reads
/// and writes it there; the fragment reads the embedded copy.
pub const SNAPSHOT: &str = "crates/darkly/releases.json";

/// One shipped version.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Release {
    /// `X.Y.Z`, without the `v` the tag carries.
    pub version: String,
    /// `YYYY-MM-DD`, UTC: the GitHub release's publish date when it is
    /// published, else the day the entry was fetched.
    pub date: String,
    /// In merge order, oldest first. Bots excluded.
    pub changes: Vec<Change>,
}

/// One pull request a release ships.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    pub pr: u32,
    pub title: String,
}

impl Release {
    pub fn parsed_version(&self) -> Version {
        self.version
            .parse()
            .unwrap_or_else(|e| panic!("invalid version in {SNAPSHOT}: {e}"))
    }

    /// The GitHub release page for this version.
    pub fn url(&self) -> String {
        format!("{REPOSITORY}/releases/tag/v{}", self.version)
    }

    /// The release notes body for the GitHub release: one line per change,
    /// title linked to its PR.
    pub fn markdown(&self) -> String {
        self.changes
            .iter()
            .map(|c| format!("- **{}** ([#{}]({}))\n", c.title, c.pr, c.url()))
            .collect()
    }
}

impl Change {
    /// The one constructor. A title carrying a URL or an en or em dash never
    /// becomes a change: AppStream fails validation on a plaintext URL inside a
    /// list item, and the dashes are forbidden in every tracked file. There is
    /// nothing to fall back to, so the fix is the PR title on GitHub.
    pub fn new(pr: u32, title: String) -> Result<Change, String> {
        for (needle, what) in [
            ("://", "a URL"),
            ("\u{2013}", "an en dash"),
            ("\u{2014}", "an em dash"),
        ] {
            if title.contains(needle) {
                return Err(format!(
                    "PR #{pr} title contains {what}, which the store listing cannot carry; \
                     fix the title on GitHub and re-run: {title:?}"
                ));
            }
        }
        Ok(Change { pr, title })
    }

    pub fn url(&self) -> String {
        format!("{REPOSITORY}/pull/{}", self.pr)
    }
}

/// [`SOURCE`], parsed once, newest first as written.
///
/// Panics on malformed input: this is shipped data that the test suite parses
/// on every run, so a broken write cannot reach a release.
pub fn releases() -> &'static [Release] {
    static RELEASES: OnceLock<Vec<Release>> = OnceLock::new();
    RELEASES.get_or_init(|| {
        serde_json::from_str(SOURCE).unwrap_or_else(|e| panic!("invalid {SNAPSHOT}: {e}"))
    })
}

/// The snapshot exactly as it is written to disk.
pub fn to_json(releases: &[Release]) -> String {
    let mut json = serde_json::to_string_pretty(releases).expect("releases serialize");
    json.push('\n');
    json
}

/// Replace the entry for `release.version` or insert it, keeping the list
/// newest first.
pub fn upsert(mut releases: Vec<Release>, release: Release) -> Vec<Release> {
    releases.retain(|r| r.version != release.version);
    releases.push(release);
    releases.sort_by(newest_first);
    releases
}

// ---------------------------------------------------------------------------
// Versions and tags
// ---------------------------------------------------------------------------

/// `X.Y.Z`. The only tag shape a release has; anything else in the tag list is
/// not a release and is ignored wherever tags are read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u32, pub u32, pub u32);

impl FromStr for Version {
    type Err = String;

    /// Accepts an optional leading `v`, so a tag name and a bare version both
    /// parse; the `v` is never stored.
    fn from_str(s: &str) -> Result<Self, String> {
        let bare = s.strip_prefix('v').unwrap_or(s);
        let mut parts = bare.split('.');
        let mut field = |name: &str| -> Result<u32, String> {
            parts
                .next()
                .filter(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
                .and_then(|p| p.parse().ok())
                .ok_or_else(|| format!("`{s}` is not X.Y.Z ({name} missing or not a number)"))
        };
        let v = Version(field("major")?, field("minor")?, field("patch")?);
        if parts.next().is_some() {
            return Err(format!("`{s}` is not X.Y.Z (too many fields)"));
        }
        Ok(v)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// The last release a commit descends from: the highest `vX.Y.Z` among the
/// tags reachable from it, minus the commit's own tag when it has one. `None`
/// for the oldest release. Tags that are not `vX.Y.Z` are ignored, which is
/// what keeps `v0.9.0-rc1` from outranking `v0.9.0` the way git's own version
/// sort would.
pub fn previous_tag(reachable: &[String], exclude: Option<Version>) -> Option<Version> {
    reachable
        .iter()
        .filter_map(|t| t.parse::<Version>().ok())
        .filter(|v| Some(*v) != exclude)
        .max()
}

/// The tag component of a `git describe --tags --long` string as a version, or
/// `None` for the tagless fallback and anything else that is not a release.
pub fn built_tag(version: &str) -> Option<Version> {
    crate::describe_parts(version).and_then(|(tag, _, _)| tag.parse().ok())
}

// ---------------------------------------------------------------------------
// Merge subjects
// ---------------------------------------------------------------------------

/// The pull requests named by a run of commit subjects, in input order, each
/// once. Recognizes the two subjects GitHub writes: `Merge pull request #N
/// from ...` (the merge button) and a subject ending in `(#N)` (the squash
/// button). Every other line is not a PR and is skipped.
pub fn pr_numbers(subjects: &str) -> Vec<u32> {
    let mut found = Vec::new();
    for line in subjects.lines() {
        let line = line.trim();
        let number = line
            .strip_prefix("Merge pull request #")
            .and_then(|rest| rest.split_once(' '))
            .filter(|(_, rest)| rest.starts_with("from "))
            .and_then(|(n, _)| n.parse().ok())
            .or_else(|| {
                line.strip_suffix(')')
                    .and_then(|s| s.rsplit_once("(#"))
                    .and_then(|(_, n)| n.parse().ok())
            });
        if let Some(n) = number {
            if !found.contains(&n) {
                found.push(n);
            }
        }
    }
    found
}

// ---------------------------------------------------------------------------
// What `gh` answers
// ---------------------------------------------------------------------------

/// `gh pr view <n> --json number,title,author`.
#[derive(Debug, serde::Deserialize)]
pub struct PullRequest {
    pub number: u32,
    pub title: String,
    pub author: Author,
}

#[derive(Debug, serde::Deserialize)]
pub struct Author {
    pub is_bot: bool,
}

/// `gh release view vX.Y.Z --json publishedAt,isDraft`.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubRelease {
    pub published_at: String,
    pub is_draft: bool,
}

impl GitHubRelease {
    /// The publish date, or `None` while the release is still a draft.
    pub fn date(&self) -> Option<&str> {
        (!self.is_draft)
            .then(|| self.published_at.get(..10))
            .flatten()
    }
}

/// Newest first, the order the snapshot keeps and AppStream requires.
pub fn newest_first(a: &Release, b: &Release) -> Ordering {
    b.parsed_version().cmp(&a.parsed_version())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn release(version: &str, changes: &[(u32, &str)]) -> Release {
        Release {
            version: version.into(),
            date: "2026-09-10".into(),
            changes: changes
                .iter()
                .map(|(pr, title)| Change::new(*pr, title.to_string()).unwrap())
                .collect(),
        }
    }

    /// The real `git log --reverse --format=%s v0.7.0..v0.8.0`, trimmed to
    /// the merge lines and the non-PR merges around them.
    const SUBJECTS: &str = "\
Merge pull request #93 from darkly-art/dependabot/cargo/dev/cargo-9fd31f0012
Merge pull request #91 from darkly-art/brush-node-dev
fix conflict
Merge pull request #95 from darkly-art/misc-bugfixes
Merge pull request #97 from darkly-art/docs
Merge pull request #100 from darkly-art/misc-bugfixes
resolve dev conflicts
Merge pull request #107 from darkly-art/smart-objects
Merge pull request #103 from darkly-art/dependabot/github_actions/dev/github-actions-674967a53d
Merge pull request #106 from darkly-art/brush-work
Merge branch 'dev' into radial-widget
Merge pull request #114 from darkly-art/brush-explorer-fleshing
Merge pull request #111 from darkly-art/better-veils
Merge pull request #113 from darkly-art/radial-widget
";

    #[test]
    fn merge_subjects_name_their_prs_in_order() {
        assert_eq!(
            pr_numbers(SUBJECTS),
            [93, 91, 95, 97, 100, 107, 103, 106, 114, 111, 113]
        );
    }

    #[test]
    fn a_squash_subject_counts_once() {
        let with_squash = format!("{SUBJECTS}Radial widget (#113)\n");
        assert_eq!(pr_numbers(&with_squash), pr_numbers(SUBJECTS));
        assert_eq!(pr_numbers("Add thing (#42)\n"), [42]);
    }

    #[test]
    fn other_merges_are_not_prs() {
        assert!(pr_numbers("Merge branch 'dev' into brush-node-dev\n").is_empty());
        assert!(pr_numbers("Merge pull request #x from nowhere\n").is_empty());
    }

    #[test]
    fn previous_tag_is_the_highest_reachable_release() {
        let all = tags(&[
            "v0.1.0", "v0.3.0", "v0.4.0", "v0.5.0", "v0.6.0", "v0.7.0", "v0.8.0",
        ]);
        assert_eq!(
            previous_tag(&all, Some(Version(0, 8, 0))),
            Some(Version(0, 7, 0))
        );
        assert_eq!(previous_tag(&all, None), Some(Version(0, 8, 0)));
        assert_eq!(
            previous_tag(
                &tags(&["v0.1.0", "v0.3.0", "v0.4.0", "v0.5.0"]),
                Some(Version(0, 5, 0))
            ),
            Some(Version(0, 4, 0))
        );
        assert_eq!(
            previous_tag(&tags(&["v0.1.0", "v0.3.0"]), Some(Version(0, 3, 0))),
            Some(Version(0, 1, 0))
        );
        assert_eq!(
            previous_tag(&tags(&["v0.1.0"]), Some(Version(0, 1, 0))),
            None
        );
    }

    /// Only `vX.Y.Z` is a release. Git's own version sort would rank a
    /// suffixed tag above the plain one; this rule does not see it at all.
    #[test]
    fn tags_that_are_not_releases_are_ignored() {
        let list = tags(&["v0.8.0", "v-next", "v0.9.0-rc1"]);
        assert_eq!(previous_tag(&list, None), Some(Version(0, 8, 0)));
    }

    #[test]
    fn versions_parse_with_or_without_the_v() {
        assert_eq!("0.8.0".parse::<Version>(), Ok(Version(0, 8, 0)));
        assert_eq!("v0.8.0".parse::<Version>(), Ok(Version(0, 8, 0)));
        assert!("0.8".parse::<Version>().is_err());
        assert!("0.10".parse::<Version>().is_err());
        assert!("v0.8.0-3-gabc".parse::<Version>().is_err());
        assert!(Version(0, 10, 0) > Version(0, 9, 0));
        assert_eq!(Version(0, 10, 0).to_string(), "0.10.0");
    }

    #[test]
    fn a_title_the_store_cannot_carry_is_refused() {
        assert!(Change::new(1, "Better veils".into()).is_ok());
        assert!(Change::new(1, "Add `code` & more".into()).is_ok());
        assert!(Change::new(1, "Try www.example.com".into()).is_ok());
        assert!(Change::new(1, "A plain - hyphen".into()).is_ok());
        for bad in [
            "See https://example.com",
            "Mirror ftp://host",
            "en \u{2013} dash",
            "em \u{2014} dash",
        ] {
            let err = Change::new(7, bad.into()).unwrap_err();
            assert!(err.contains("#7"), "{err}");
        }
    }

    #[test]
    fn markdown_links_each_change_to_its_pr() {
        let r = release("0.8.0", &[(111, "Better veils"), (113, "Radial Widget")]);
        assert_eq!(
            r.markdown(),
            "- **Better veils** (\
             [#111](https://github.com/darkly-art/darkly/pull/111))\n\
             - **Radial Widget** (\
             [#113](https://github.com/darkly-art/darkly/pull/113))\n"
        );
        assert_eq!(
            r.url(),
            "https://github.com/darkly-art/darkly/releases/tag/v0.8.0"
        );
    }

    #[test]
    fn upsert_keeps_newest_first_and_replaces_in_place() {
        let list = vec![release("0.8.0", &[]), release("0.7.0", &[])];
        let list = upsert(list, release("0.9.0", &[]));
        let versions: Vec<&str> = list.iter().map(|r| r.version.as_str()).collect();
        assert_eq!(versions, ["0.9.0", "0.8.0", "0.7.0"]);

        let list = upsert(list, release("0.8.0", &[(1, "replaced")]));
        assert_eq!(list.len(), 3);
        assert_eq!(list[1].changes[0].title, "replaced");
    }

    #[test]
    fn built_tag_reads_the_describe_string() {
        assert_eq!(built_tag("v0.8.0-3-gabc1234"), Some(Version(0, 8, 0)));
        assert_eq!(built_tag("0.0.0-0-gunknown"), Some(Version(0, 0, 0)));
        assert_eq!(built_tag("nonsense"), None);
    }

    /// A tagged checkout whose snapshot stops before its own tag is a release
    /// shipping without notes. Local and post-tag: the CI gate in `ci.yml`'s
    /// `packaging` job is the one that fires before the tag exists.
    #[test]
    fn newest_snapshot_entry_is_not_older_than_the_built_tag() {
        let built = built_tag(crate::VERSION).expect("VERSION has the describe shape");
        // The tagless fallback stamps `0.0.0`, which any snapshot satisfies.
        let newest = releases().first().map(Release::parsed_version);
        assert!(
            newest >= Some(built),
            "snapshot is behind the tag {built}; run `cargo releases --fetch X.Y.Z`"
        );
    }

    #[test]
    fn the_committed_snapshot_is_ordered_and_canonical() {
        let list = releases();
        assert!(list
            .windows(2)
            .all(|w| newest_first(&w[0], &w[1]) == Ordering::Less));
        assert_eq!(
            to_json(list),
            SOURCE,
            "releases.json is not what to_json writes"
        );
    }

    /// The `gh` shapes the fetch decodes, as captured from the real commands,
    /// so a renamed field fails here rather than only at the keyboard.
    #[test]
    fn gh_output_decodes() {
        let pr: PullRequest = serde_json::from_str(
            r#"{"author":{"id":"MDQ6VXNlcjIwMjYxNjk5","is_bot":false,"login":"TheTechromancer","name":""},"number":111,"title":"Better veils"}"#,
        )
        .unwrap();
        assert_eq!(
            (pr.number, pr.title.as_str(), pr.author.is_bot),
            (111, "Better veils", false)
        );

        let bot: PullRequest = serde_json::from_str(
            r#"{"author":{"is_bot":true,"login":"app/dependabot"},"number":116,"title":"Bump the cargo group across 1 directory with 9 updates"}"#,
        )
        .unwrap();
        assert!(bot.author.is_bot);

        let rel: GitHubRelease =
            serde_json::from_str(r#"{"isDraft":false,"publishedAt":"2026-09-10T21:45:29Z"}"#)
                .unwrap();
        assert_eq!(rel.date(), Some("2026-09-10"));
        let draft: GitHubRelease =
            serde_json::from_str(r#"{"isDraft":true,"publishedAt":"2026-09-10T21:45:29Z"}"#)
                .unwrap();
        assert_eq!(draft.date(), None);
    }
}
