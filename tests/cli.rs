// integration tests

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn temp_crate(source: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace]\n",
    )
    .unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    let lib = dir.path().join("src/lib.rs");
    std::fs::write(&lib, source).unwrap();
    (dir, lib)
}

fn autorust(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_autorust"))
        .arg("--manifest-path")
        .arg(dir)
        .args(["--color", "never"])
        .args(args)
        .output()
        .unwrap()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[test]
fn fixes_basic_fixture() {
    let (dir, lib) = temp_crate(&fixture("basic.rs"));
    let out = autorust(dir.path(), &["--allow-no-vcs"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        std::fs::read_to_string(&lib).unwrap(),
        fixture("basic.fixed.rs")
    );

    let out = autorust(dir.path(), &["--check"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(stderr(&out).contains("nothing to fix"));
}

#[test]
fn dry_run_prints_diff_and_writes_nothing() {
    let source = fixture("basic.rs");
    let (dir, lib) = temp_crate(&source);
    let out = autorust(dir.path(), &["--dry-run"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));

    let diff = String::from_utf8(out.stdout).unwrap();
    assert!(
        diff.starts_with("--- a/src/lib.rs\n+++ b/src/lib.rs\n"),
        "{diff}"
    );
    assert!(
        diff.contains("-    v.len() == 0\n+    v.is_empty()\n"),
        "{diff}"
    );
    assert_eq!(std::fs::read_to_string(&lib).unwrap(), source);
}

#[test]
fn check_mode_fails_when_there_is_something_to_fix() {
    let source = fixture("basic.rs");
    let (dir, lib) = temp_crate(&source);
    let out = autorust(dir.path(), &["--check"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(stderr(&out).contains("[clippy::needless_return]"));
    assert_eq!(std::fs::read_to_string(&lib).unwrap(), source);
}

#[test]
fn lint_filter() {
    let (dir, lib) = temp_crate(&fixture("basic.rs"));
    let out = autorust(dir.path(), &["--allow-no-vcs", "--lint", "len_zero"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));

    let fixed = std::fs::read_to_string(&lib).unwrap();
    assert!(fixed.contains("v.is_empty()"));
    assert!(
        fixed.contains("return a + b;"),
        "other lints must stay untouched"
    );
}

#[test]
fn refuses_without_vcs() {
    let source = fixture("basic.rs");
    let (dir, lib) = temp_crate(&source);
    let out = autorust(dir.path(), &[]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("--allow-no-vcs"));
    assert_eq!(std::fs::read_to_string(&lib).unwrap(), source);
}

#[test]
fn refuses_broken_crate() {
    let (dir, _lib) = temp_crate("pub fn f() -> i32 { undefined_name }\n");
    let out = autorust(dir.path(), &["--allow-no-vcs"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        stderr(&out).contains("does not compile"),
        "{}",
        stderr(&out)
    );
}
