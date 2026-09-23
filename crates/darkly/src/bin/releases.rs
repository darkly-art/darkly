//! Write one release into `crates/darkly/releases.json`, or print one as
//! GitHub release notes.
//!
//! ```text
//! cargo releases --fetch 0.9.0    # resolve the range, read titles, write the entry
//! cargo releases --notes v0.9.0   # print the entry as markdown for `gh release create`
//! ```
//!
//! `--fetch` is the one command in this repository that touches the network:
//! it shells out to `git` for the tag graph and to `gh` for PR titles and the
//! release's publish date. Every decision it makes is a function in
//! [`darkly::releases`] over the text those commands print; this file only
//! runs them in order. `--notes` is offline and reads the embedded snapshot,
//! which is what `docs-artifact.yml` runs on the tagged tree.

use std::path::Path;
use std::process::{Command, ExitCode};

use darkly::docs_md;
use darkly::releases::{
    self, pr_numbers, previous_tag, upsert, Change, GitHubRelease, PullRequest, Release, Version,
};

const HELP: &str = "\
releases - maintain the release history the store listing is generated from

USAGE:
    releases --fetch <version>    Resolve the release's commit range from the
                                  v* tags, read the titles of the PRs merged in
                                  it, and write the entry to releases.json.
                                  Needs `git` and an authenticated `gh`.
    releases --notes <version>    Print the entry as markdown release notes.
    -h, --help                    Show this message.

<version> is X.Y.Z, with or without a leading v.
";

enum Mode {
    Fetch(Version),
    Notes(Version),
}

fn parse_args() -> Result<Mode, String> {
    let mut argv = std::env::args().skip(1);
    let mode = match argv.next().as_deref() {
        Some("--fetch") => Mode::Fetch(version_arg(argv.next())?),
        Some("--notes") => Mode::Notes(version_arg(argv.next())?),
        Some("-h" | "--help") => {
            print!("{HELP}");
            std::process::exit(0);
        }
        Some(other) => return Err(format!("unrecognized argument `{other}`")),
        None => return Err("one of --fetch or --notes is required".into()),
    };
    if let Some(extra) = argv.next() {
        return Err(format!("unexpected argument `{extra}`"));
    }
    Ok(mode)
}

fn version_arg(arg: Option<String>) -> Result<Version, String> {
    arg.ok_or("a version is required")?.parse()
}

/// Run a command and hand back its stdout. Stderr passes through, and a
/// non-zero exit is an error naming the command, so `gh` and `git` explain
/// their own failures.
fn run(program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| format!("cannot run `{program}`: {e}"))?;
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
    if !output.status.success() {
        return Err(format!("`{program} {}` failed", args.join(" ")));
    }
    String::from_utf8(output.stdout).map_err(|e| format!("`{program}` wrote non-UTF-8: {e}"))
}

fn gh_json<T: serde::de::DeserializeOwned>(args: &[&str]) -> Result<T, String> {
    let text = run("gh", args)?;
    serde_json::from_str(&text).map_err(|e| format!("cannot decode `gh {}`: {e}", args.join(" ")))
}

fn read_snapshot(path: &Path) -> Result<Vec<Release>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn fetch(version: Version, path: &Path) -> Result<(), String> {
    let tag = format!("v{version}");

    // The target is the tag when it exists (seeding history), else HEAD.
    let all_tags = run("git", &["tag", "--list", "v*"])?;
    let seeding = all_tags.lines().any(|t| t == tag);
    let target = if seeding { tag.as_str() } else { "HEAD" };

    // The previous release is the highest one the target descends from.
    let reachable: Vec<String> = run("git", &["tag", "--merged", target, "--list", "v*"])?
        .lines()
        .map(str::to_string)
        .collect();
    let previous = previous_tag(&reachable, seeding.then_some(version));
    if let Some(prev) = previous {
        if version <= prev {
            return Err(format!(
                "{version} is not newer than v{prev}, the last release reachable from {target}"
            ));
        }
    }
    let range = match previous {
        Some(prev) => format!("v{prev}..{target}"),
        None => target.to_string(),
    };

    let mut snapshot = read_snapshot(path)?;
    if let Some(newer) = snapshot.iter().find(|r| r.parsed_version() > version) {
        eprintln!(
            "warning: releases.json already holds {} above {version}; is the version right?",
            newer.version
        );
    }

    let subjects = run("git", &["log", "--reverse", "--format=%s", &range])?;
    let numbers = pr_numbers(&subjects);
    if numbers.is_empty() {
        eprintln!("warning: no pull requests merged in {range}");
    }

    let mut changes = Vec::new();
    for n in &numbers {
        let pr: PullRequest = gh_json(&[
            "pr",
            "view",
            &n.to_string(),
            "--json",
            "number,title,author",
        ])?;
        if pr.author.is_bot {
            println!("skip  #{n} (bot)");
            continue;
        }
        println!("add   #{n} {}", pr.title);
        changes.push(Change::new(pr.number, pr.title)?);
    }

    // Published releases keep their publish date; anything else is dated today.
    let published: Option<GitHubRelease> =
        gh_json(&["release", "view", &tag, "--json", "publishedAt,isDraft"]).ok();
    let date = match published.as_ref().and_then(GitHubRelease::date) {
        Some(d) => d.to_string(),
        None => run("date", &["-u", "+%F"])?.trim().to_string(),
    };

    let release = Release {
        version: version.to_string(),
        date,
        changes,
    };
    snapshot = upsert(snapshot, release);
    std::fs::write(path, releases::to_json(&snapshot))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    println!(
        "wrote {version} ({range}, {} changes) to {}; now run `cargo sync-docs`",
        numbers.len(),
        path.display()
    );
    Ok(())
}

fn notes(version: Version) -> Result<(), String> {
    let wanted = version.to_string();
    let release = releases::releases()
        .iter()
        .find(|r| r.version == wanted)
        .ok_or_else(|| {
            format!(
                "no entry for {version} in releases.json; run `cargo releases --fetch {version}`"
            )
        })?;
    print!("{}", release.markdown());
    Ok(())
}

fn main() -> ExitCode {
    let result = match parse_args() {
        Ok(Mode::Fetch(v)) => fetch(v, &docs_md::repo_root().join(releases::SNAPSHOT)),
        Ok(Mode::Notes(v)) => notes(v),
        Err(e) => Err(format!("{e}\n\n{HELP}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("releases: {e}");
            ExitCode::FAILURE
        }
    }
}
