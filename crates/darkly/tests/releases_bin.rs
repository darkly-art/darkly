//! The `releases` binary's offline half, run as CI runs it.
//!
//! `docs-artifact.yml` calls `releases --notes <tag>` on the tagged tree and
//! feeds the output to `gh release create`. This runs the real binary, so what
//! is checked is what that job gets: the markdown for a committed entry, and a
//! failure for a version the snapshot lacks.
//!
//! Run with: `cargo test -p darkly --test releases_bin`

use std::process::Command;

use darkly::releases::releases;

fn notes(version: &str) -> (bool, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_releases"))
        .args(["--notes", version])
        .output()
        .expect("failed to run releases");
    (
        output.status.success(),
        String::from_utf8(output.stdout).expect("releases wrote non-UTF-8"),
    )
}

#[test]
fn notes_prints_the_committed_entry() {
    let newest = releases().first().expect("the snapshot has an entry");
    let (ok, out) = notes(&format!("v{}", newest.version));
    assert!(ok);
    assert_eq!(out, newest.markdown());
    assert!(
        out.contains("/pull/"),
        "each change links to its PR:\n{out}"
    );
}

#[test]
fn notes_fails_for_a_version_the_snapshot_lacks() {
    let (ok, out) = notes("v9.9.9");
    assert!(!ok);
    assert!(out.is_empty());
}
