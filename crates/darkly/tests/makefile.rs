//! The root Makefile's `MODE` switch, dry-run with `make -n`.
//!
//! `MODE` picks the frontend's deploy flavor for `make frontend`. Vite builds
//! any mode other than `app` as the demo, so the Makefile refuses a value it
//! does not know rather than let a typo ship the wrong site.
//!
//! Run with: `cargo test -p darkly --test makefile`

use std::process::{Command, Output};

fn dry_run(args: &[&str]) -> Output {
    Command::new("make")
        .arg("-n")
        .arg("--no-print-directory")
        .args(args)
        .current_dir(darkly::docs_md::repo_root())
        .output()
        .expect("make runs")
}

fn vite_line(out: &Output) -> String {
    assert!(
        out.status.success(),
        "make failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find(|l| l.contains("vite build"))
        .expect("the frontend recipe runs vite build")
        .to_string()
}

#[test]
fn frontend_builds_the_app_by_default() {
    assert!(vite_line(&dry_run(&["frontend"])).ends_with("--mode app"));
}

#[test]
fn frontend_builds_the_mode_it_is_given() {
    assert!(vite_line(&dry_run(&["frontend", "MODE=demo"])).ends_with("--mode demo"));
}

#[test]
fn an_unknown_mode_is_refused() {
    for mode in ["MODE=bogus", "MODE=app demo", "MODE="] {
        let out = dry_run(&["frontend", mode]);
        assert!(!out.status.success(), "{mode} was accepted");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("MODE must be app or demo"),
            "{mode}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
