//! The release scripts, run against throwaway repositories.
//!
//! `scripts/metainfo-releases.sh` renders the store listing's release history
//! from tags, and `scripts/release.sh` cuts a release. Both are run as a
//! maintainer or CI runs them: the real script, a real git repository, and for
//! the release script a `gh` shim that answers from a JSON fixture through the
//! same `jq` expression gh would apply and records what it was asked to do.
//!
//! Every git invocation is isolated from the developer's own configuration, so
//! a `tag.gpgSign` or a missing identity cannot change the outcome.
//!
//! Run with: `cargo test -p darkly --test release_scripts`

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};

fn repo_root() -> PathBuf {
    darkly::docs_md::repo_root()
}

/// A directory under the system temp dir, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "darkly-release-scripts-{}-{}-{name}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A command with git's configuration and identity pinned.
fn isolated(program: &str, dir: &Path) -> Command {
    let mut cmd = Command::new(program);
    cmd.current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .env("GIT_AUTHOR_DATE", "2026-01-01T12:00:00Z")
        .env("GIT_COMMITTER_DATE", "2026-01-01T12:00:00Z");
    cmd
}

fn git_at(dir: &Path, date: &str, args: &[&str]) -> String {
    let out = isolated("git", dir)
        .env("GIT_COMMITTER_DATE", date)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout)
        .unwrap()
        .trim_end()
        .to_string()
}

fn git(dir: &Path, args: &[&str]) -> String {
    git_at(dir, "2026-01-01T12:00:00Z", args)
}

fn commit(dir: &Path, msg: &str) -> String {
    git(dir, &["commit", "-q", "--allow-empty", "-m", msg]);
    git(dir, &["rev-parse", "HEAD"])
}

fn annotated_tag(dir: &Path, tag: &str, message: &str, date: &str) {
    git_at(dir, date, &["tag", "-a", tag, "-m", message]);
}

fn stdout(out: &Output) -> String {
    String::from_utf8(out.stdout.clone()).unwrap()
}

// ---- metainfo-releases.sh ------------------------------------------------

const METAINFO: &str = "\
<?xml version=\"1.0\" encoding=\"UTF-8\"?>
<component type=\"desktop-application\">
  <id>art.example.App</id>
</component>
";

fn render(dir: &Path, metainfo: &str) -> Output {
    let file = dir.join("in.metainfo.xml");
    fs::write(&file, metainfo).unwrap();
    isolated("bash", dir)
        .arg(repo_root().join("scripts/metainfo-releases.sh"))
        .arg(&file)
        .output()
        .unwrap()
}

#[test]
fn renders_reachable_release_tags_newest_first() {
    let s = Scratch::new("render");
    let d = &s.0;
    git(d, &["init", "-q", "-b", "dev"]);

    commit(d, "one");
    git_at(d, "2026-02-03T10:00:00Z", &["tag", "v0.1.0"]); // lightweight
    commit(d, "two");
    annotated_tag(
        d,
        "v0.2.0",
        "v0.2.0\n\nFix & tidy (#4)\n\nAdd <thing> (#5)\n",
        "2026-03-04T10:00:00Z",
    );
    commit(d, "three");
    annotated_tag(d, "v0.3.0-rc1", "rc", "2026-03-05T10:00:00Z");
    git(d, &["tag", "next"]);

    // A release on a branch HEAD does not contain never appears.
    git(d, &["switch", "-q", "-c", "side"]);
    commit(d, "side");
    annotated_tag(
        d,
        "v0.4.0",
        "v0.4.0\n\nElsewhere (#9)",
        "2026-04-01T10:00:00Z",
    );
    git(d, &["switch", "-q", "dev"]);

    let out = render(d, METAINFO);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        stdout(&out),
        "\
<?xml version=\"1.0\" encoding=\"UTF-8\"?>
<component type=\"desktop-application\">
  <id>art.example.App</id>
  <releases>
    <release version=\"0.2.0\" date=\"2026-03-04\">
      <url type=\"details\">https://github.com/darkly-art/darkly/releases/tag/v0.2.0</url>
      <description>
        <ul>
          <li>Fix &amp; tidy (#4)</li>
          <li>Add &lt;thing&gt; (#5)</li>
        </ul>
      </description>
    </release>
    <release version=\"0.1.0\" date=\"2026-01-01\">
      <url type=\"details\">https://github.com/darkly-art/darkly/releases/tag/v0.1.0</url>
    </release>
  </releases>

</component>
"
    );
}

#[test]
fn renders_an_empty_block_outside_a_repository() {
    let s = Scratch::new("norepo");
    let out = render(&s.0, METAINFO);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout(&out).contains("  <releases>\n  </releases>\n\n</component>\n"));
}

const WITH_BLOCK: &str = "\
<component>
  <id>art.example.App</id>
  <!-- kept -->
  <releases>
    <release version=\"0.0.1\" date=\"2025-01-01\">
    </release>
  </releases>

</component>
";

#[test]
fn replaces_an_existing_block_and_keeps_what_surrounds_it() {
    let s = Scratch::new("replace");
    let d = &s.0;
    git(d, &["init", "-q", "-b", "dev"]);
    commit(d, "one");
    annotated_tag(d, "v0.1.0", "v0.1.0\n\nFirst (#1)", "2026-02-03T10:00:00Z");

    let out = render(d, WITH_BLOCK);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let rendered = stdout(&out);
    assert_eq!(
        rendered,
        "\
<component>
  <id>art.example.App</id>
  <!-- kept -->
  <releases>
    <release version=\"0.1.0\" date=\"2026-02-03\">
      <url type=\"details\">https://github.com/darkly-art/darkly/releases/tag/v0.1.0</url>
      <description>
        <ul>
          <li>First (#1)</li>
        </ul>
      </description>
    </release>
  </releases>

</component>
"
    );

    // Rendering its own output changes nothing.
    let again = render(d, &rendered);
    assert_eq!(stdout(&again), rendered);
}

#[test]
fn keeps_the_committed_block_when_there_are_no_tags() {
    let s = Scratch::new("notags");
    let out = render(&s.0, WITH_BLOCK);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(stdout(&out), WITH_BLOCK);
}

/// A release tarball: no `.git`, the committed block from before its own
/// release, and a `version.txt` git archive filled in at the release tag.
#[test]
fn adds_a_tarballs_own_release_from_its_version_file() {
    let s = Scratch::new("tarball");
    let d = &s.0;
    fs::create_dir_all(d.join("scripts")).unwrap();
    fs::create_dir_all(d.join("crates/darkly")).unwrap();
    fs::copy(
        repo_root().join("scripts/metainfo-releases.sh"),
        d.join("scripts/metainfo-releases.sh"),
    )
    .unwrap();
    let version_file = d.join("crates/darkly/version.txt");
    let file = d.join("in.metainfo.xml");
    fs::write(&file, WITH_BLOCK).unwrap();
    let run = || {
        isolated("bash", d)
            .arg(d.join("scripts/metainfo-releases.sh"))
            .arg(&file)
            .output()
            .unwrap()
    };

    fs::write(&version_file, "# filled in\nv0.2.0 abc1234 2026-03-04\n").unwrap();
    let out = run();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        stdout(&out),
        "\
<component>
  <id>art.example.App</id>
  <!-- kept -->
  <releases>
    <release version=\"0.2.0\" date=\"2026-03-04\">
      <url type=\"details\">https://github.com/darkly-art/darkly/releases/tag/v0.2.0</url>
    </release>
    <release version=\"0.0.1\" date=\"2025-01-01\">
    </release>
  </releases>

</component>
"
    );

    // Placeholders (a checkout), a build past a tag, and a release the block
    // already lists all leave it as committed.
    for line in [
        "$Format:%(describe:tags)$ $Format:%h$ $Format:%cs$",
        "v0.2.0-3-gabc1234 abc1234 2026-03-04",
        "v0.0.1 abc1234 2025-01-01",
    ] {
        fs::write(&version_file, format!("# filled in\n{line}\n")).unwrap();
        assert_eq!(stdout(&run()), WITH_BLOCK, "{line}");
    }
}

#[test]
fn refuses_a_malformed_releases_block() {
    let s = Scratch::new("malformed");
    for bad in [
        "<component>\n  <releases>\n  </releases>\n  <releases>\n  </releases>\n</component>\n",
        "<component>\n  <releases/>\n</component>\n",
        "<component>\n  <releases>\n</component>\n",
    ] {
        let out = render(&s.0, bad);
        assert!(!out.status.success(), "accepted: {bad}");
        assert!(out.stdout.is_empty());
    }
}

#[test]
fn refuses_without_a_closing_component() {
    let s = Scratch::new("noclose");
    let out = render(&s.0, "<component>\n  <id>x</id>\n");
    assert!(!out.status.success());
    assert!(out.stdout.is_empty());

    let out = render(&s.0, "<component><id>x</id></component>\n");
    assert!(
        !out.status.success(),
        "</component> must be on its own line"
    );
    assert!(out.stdout.is_empty());
}

// ---- release.sh ----------------------------------------------------------

const METAINFO_PATH: &str = "packaging/art.darkly.Darkly.metainfo.xml";

/// Answers each query from a JSON fixture through the `--jq` expression the
/// script passes, the way gh itself would: merged PRs from `prs.json`, the
/// open release PR from `open.json`, its checks from `checks.json`, and its
/// head commit from `head` (the work tree's HEAD when absent). Logs every call
/// and the body of `pr create` / `pr edit`.
const GH_SHIM: &str = r#"#!/usr/bin/env bash
set -euo pipefail
echo "$*" >> "$SHIM_DIR/log"
jq_expr=""
args=("$@")
for i in "${!args[@]}"; do
  [ "${args[$i]}" = "--jq" ] && jq_expr="${args[$((i+1))]}"
done
case "$*" in
  "pr list --state merged"*) jq -r "$jq_expr" "$SHIM_DIR/prs.json" ;;
  "pr list --base master"*)  jq -r "$jq_expr" "$SHIM_DIR/open.json" ;;
  "pr view"*)
    head=$(cat "$SHIM_DIR/head" 2>/dev/null || git rev-parse HEAD)
    jq -rn --arg h "$head" '{headRefOid: $h}' | jq -r "$jq_expr" ;;
  "pr checks"*)              jq -r "$jq_expr" "$SHIM_DIR/checks.json" ;;
  "pr merge"*)               : ;;
  "pr create"*|"pr edit"*)   cat > "$SHIM_DIR/body"; echo "https://example.invalid/pull/1" ;;
  *) echo "gh shim: unexpected: $*" >&2; exit 2 ;;
esac
"#;

struct Release {
    _s: Scratch,
    origin: PathBuf,
    work: PathBuf,
    shim: PathBuf,
}

impl Release {
    /// A bare `origin` with `dev` at an annotated `v0.1.0` plus the commits
    /// named in `after`, pushed, and a clone of it on `dev`.
    fn new(name: &str, after: &[&str]) -> (Self, Vec<String>, String) {
        let s = Scratch::new(name);
        let origin = s.0.join("origin.git");
        let work = s.0.join("work");
        let shim = s.0.join("shim");
        fs::create_dir_all(&shim).unwrap();
        fs::create_dir_all(&work).unwrap();
        git(&s.0, &["init", "-q", "--bare", "-b", "dev", "origin.git"]);
        git(&work, &["init", "-q", "-b", "dev"]);
        git(
            &work,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        fs::create_dir_all(work.join("packaging")).unwrap();
        fs::write(work.join(METAINFO_PATH), METAINFO).unwrap();
        git(&work, &["add", "packaging"]);
        let before = commit(&work, "before the release");
        annotated_tag(&work, "v0.1.0", "v0.1.0", "2026-01-01T12:00:00Z");
        let shas = after.iter().map(|m| commit(&work, m)).collect();
        git(&work, &["push", "-q", "origin", "dev", "--tags"]);
        git(&work, &["fetch", "-q", "origin"]);

        fs::write(shim.join("gh"), GH_SHIM).unwrap();
        Command::new("chmod")
            .arg("+x")
            .arg(shim.join("gh"))
            .status()
            .unwrap();
        fs::write(shim.join("open.json"), r#"[{"number":42}]"#).unwrap();
        fs::write(
            shim.join("checks.json"),
            r#"[{"name":"test","bucket":"pass"},{"name":"docs","bucket":"skipping"}]"#,
        )
        .unwrap();
        fs::write(shim.join("log"), "").unwrap();
        (
            Release {
                _s: s,
                origin,
                work,
                shim,
            },
            shas,
            before,
        )
    }

    fn prs(&self, prs: &[(u32, &str, bool, &str)]) {
        let json: Vec<String> = prs
            .iter()
            .map(|(n, title, bot, oid)| {
                format!(
                    r#"{{"number":{n},"title":"{title}","author":{{"is_bot":{bot},"login":"x"}},"mergeCommit":{{"oid":"{oid}"}},"url":"https://github.com/o/r/pull/{n}"}}"#
                )
            })
            .collect();
        fs::write(self.shim.join("prs.json"), format!("[{}]", json.join(","))).unwrap();
    }

    fn run(&self, version: &str) -> Output {
        let path = format!(
            "{}:{}",
            self.shim.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        isolated("bash", &self.work)
            .arg(repo_root().join("scripts/release.sh"))
            .args([version, "--yes"])
            .env("PATH", path)
            .env("SHIM_DIR", &self.shim)
            .output()
            .unwrap()
    }

    fn log(&self) -> String {
        fs::read_to_string(self.shim.join("log")).unwrap()
    }

    fn origin_has(&self, tag: &str) -> bool {
        isolated("git", &self.origin)
            .args(["rev-parse", "-q", "--verify", &format!("refs/tags/{tag}")])
            .output()
            .unwrap()
            .status
            .success()
    }
}

#[test]
fn tags_from_merged_prs_and_writes_the_pr_body() {
    let (r, shas, before) = Release::new("tag", &["human pr", "bot pr", "direct"]);
    r.prs(&[
        (6, "Bump deps", true, &shas[1]),
        (5, "Human change", false, &shas[0]),
        (3, "Already released", false, &before),
    ]);

    let out = r.run("0.2.0");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    assert!(r.origin_has("v0.2.0"));
    let fmt = |f: &str| {
        git(
            &r.origin,
            &["for-each-ref", &format!("--format={f}"), "refs/tags/v0.2.0"],
        )
    };
    assert_eq!(fmt("%(objecttype)"), "tag");
    assert_eq!(fmt("%(contents:subject)"), "v0.2.0");
    assert_eq!(fmt("%(contents:body)"), "Human change (#5)");

    let log = r.log();
    assert!(log.contains("pr edit 42 --title Dev -> Master 0.2.0 --body-file -"));
    assert!(log.contains("pr merge 42 --merge"));
    assert!(!log.contains("pr create"));
    assert_eq!(
        fs::read_to_string(r.shim.join("body")).unwrap(),
        "- https://github.com/o/r/pull/5\n"
    );

    // The checked-in release history is refreshed and left for the maintainer
    // to commit.
    let metainfo = fs::read_to_string(r.work.join(METAINFO_PATH)).unwrap();
    assert!(metainfo.contains("<release version=\"0.2.0\""));
    assert!(metainfo.contains("<li>Human change (#5)</li>"));
    assert_eq!(
        git(&r.work, &["status", "--porcelain"]),
        format!(" M {METAINFO_PATH}")
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("git commit"));
}

#[test]
fn opens_a_release_pr_and_stops_when_none_is_open() {
    let (r, shas, _) = Release::new("nopr", &["human pr"]);
    r.prs(&[(5, "Human change", false, &shas[0])]);
    fs::write(r.shim.join("open.json"), "[]").unwrap();

    let out = r.run("0.2.0");
    assert!(!out.status.success());
    assert!(!r.origin_has("v0.2.0"));
    assert!(r
        .log()
        .contains("pr create --base master --head dev --title Dev -> Master 0.2.0 --body-file -"));
}

#[test]
fn refuses_while_the_release_pr_is_not_green() {
    let (r, shas, _) = Release::new("red", &["human pr"]);
    r.prs(&[(5, "Human change", false, &shas[0])]);
    fs::write(
        r.shim.join("checks.json"),
        r#"[{"name":"test","bucket":"pass"},{"name":"clippy","bucket":"pending"}]"#,
    )
    .unwrap();

    let out = r.run("0.2.0");
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("clippy: pending"));
    assert!(!r.origin_has("v0.2.0"));
    assert!(!r.log().contains("pr merge"));
}

#[test]
fn refuses_when_the_release_pr_is_not_at_head() {
    let (r, shas, _) = Release::new("stale", &["human pr"]);
    r.prs(&[(5, "Human change", false, &shas[0])]);
    fs::write(r.shim.join("head"), "0".repeat(40)).unwrap();

    let out = r.run("0.2.0");
    assert!(!out.status.success());
    assert!(!r.origin_has("v0.2.0"));
}

#[test]
fn a_title_the_store_cannot_carry_is_refused() {
    let (r, shas, _) = Release::new("url", &["human pr"]);
    r.prs(&[(5, "See https://example.com", false, &shas[0])]);

    let out = r.run("0.2.0");
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("(#5)"));
    assert!(!r.origin_has("v0.2.0"));
    assert!(!r.log().contains("pr create"));
}

#[test]
fn refuses_before_anything_irreversible() {
    // Not above the last release.
    let (r, shas, _) = Release::new("below", &["human pr"]);
    r.prs(&[(5, "Human change", false, &shas[0])]);
    assert!(!r.run("0.0.9").status.success());
    assert!(!r.origin_has("v0.0.9"));
    assert_eq!(r.log(), "");

    // An uncommitted change to a tracked file.
    let (r, shas, _) = Release::new("dirty", &["human pr"]);
    r.prs(&[(5, "Human change", false, &shas[0])]);
    fs::write(r.work.join(METAINFO_PATH), "edited").unwrap();
    assert!(!r.run("0.2.0").status.success());
    assert!(!r.origin_has("v0.2.0"));
    assert_eq!(r.log(), "");

    // HEAD ahead of origin/dev.
    let (r, shas, _) = Release::new("ahead", &["human pr"]);
    r.prs(&[(5, "Human change", false, &shas[0])]);
    commit(&r.work, "unpushed");
    assert!(!r.run("0.2.0").status.success());
    assert!(!r.origin_has("v0.2.0"));
    assert_eq!(r.log(), "");
}

#[test]
fn untracked_files_do_not_block_a_release() {
    let (r, shas, _) = Release::new("untracked", &["human pr"]);
    r.prs(&[(5, "Human change", false, &shas[0])]);
    fs::write(r.work.join("notes.md"), "scratch").unwrap();
    let out = r.run("0.2.0");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(r.origin_has("v0.2.0"));
}

#[test]
fn refuses_when_the_pr_list_is_truncated() {
    let (r, shas, _) = Release::new("truncated", &["human pr"]);
    // A full page whose oldest entry is still inside the range.
    let mut prs: Vec<(u32, String, bool, String)> = (0..199)
        .map(|i| {
            (
                1000 + i,
                format!("Later {i}"),
                false,
                format!("{:040x}", i + 1),
            )
        })
        .collect();
    prs.push((5, "Human change".into(), false, shas[0].clone()));
    let borrowed: Vec<(u32, &str, bool, &str)> = prs
        .iter()
        .map(|(n, t, b, o)| (*n, t.as_str(), *b, o.as_str()))
        .collect();
    r.prs(&borrowed);

    let out = r.run("0.2.0");
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("truncated"));
    assert!(!r.origin_has("v0.2.0"));
}
