// runner.rs

use anyhow::{Context, Result, bail};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
};

#[derive(Debug)]
pub struct ClippyRun {
    pub lines: String,
    pub exit_status: ExitStatus,
    pub err: String,
}

// lookup cargo in env
fn cargo() -> OsString {
    std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into())
}

pub fn manifest_file(path: &Path) -> Result<PathBuf> {
    let manifest = if path.is_dir() {
        path.join("Cargo.toml")
    } else {
        path.to_path_buf()
    };
    if !manifest.is_file() {
        bail!("manifest not found: {}", manifest.display());
    }
    manifest
        .canonicalize()
        .with_context(|| format!("can't resolve {}", manifest.display()))
}

fn manifest_dir(manifest: &Path) -> &Path {
    manifest.parent().unwrap_or(Path::new("."))
}

pub fn workspace_root(manifest: &Path) -> Result<PathBuf> {
    let output = Command::new(cargo())
        .args([
            "metadata",
            "--no-deps",
            "--format-version",
            "1",
            "--manifest-path",
        ])
        .arg(manifest)
        .current_dir(manifest_dir(manifest))
        .output()
        .context("failed to launch `cargo metadata`")?;

    if !output.status.success() {
        bail!(
            "`cargo metadata` failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[derive(serde::Deserialize)]
    struct Metadata {
        workspace_root: PathBuf,
    }
    let meta: Metadata =
        serde_json::from_slice(&output.stdout).context("can't parse `cargo metadata` output")?;
    meta.workspace_root
        .canonicalize()
        .context("can't resolve workspace root")
}

// запускаем клиппи и получаем жсон
// run clippy to get json
pub fn run_clippy(manifest: &Path, extra_args: &[String]) -> Result<ClippyRun> {
    let output = Command::new(cargo())
        .args([
            "clippy",
            "--all-targets",
            "--message-format=json",
            "--manifest-path",
        ])
        .arg(manifest)
        .args(extra_args)
        .current_dir(manifest_dir(manifest))
        .output()
        .with_context(|| format!("cargo not launch succesfully for {}", manifest.display()))?;

    let stdout =
        String::from_utf8(output.stdout).context("cargo produced invalid UTF-8 on stdout")?;

    let result = ClippyRun {
        lines: stdout,
        exit_status: output.status,
        err: String::from_utf8_lossy(&output.stderr).into_owned(),
    };

    Ok(result)
}

// проверяем поломали ли мы чета после применения изменений
pub fn check_builds(manifest: &Path, extra_args: &[String]) -> Result<Option<String>> {
    let output = Command::new(cargo())
        .args(["check", "--all-targets", "--quiet", "--manifest-path"])
        .arg(manifest)
        .args(extra_args.iter().take_while(|a| *a != "--"))
        .current_dir(manifest_dir(manifest))
        .output()
        .context("failed to launch `cargo check`")?;

    if output.status.success() {
        Ok(None)
    } else {
        Ok(Some(String::from_utf8_lossy(&output.stderr).into_owned()))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum VcsState {
    NoVcs,
    Clean,
    Changed {
        staged: Vec<String>,
        dirty: Vec<String>,
    },
}

pub fn vcs_state(dir: &Path) -> Result<VcsState> {
    let inside = Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(dir)
        .output();
    match inside {
        Ok(o) if o.status.success() => {}
        // нет информации о гите
        _ => return Ok(VcsState::NoVcs),
    }

    let output = Command::new("git")
        .args([
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
            "--",
            ".",
        ])
        .current_dir(dir)
        .output()
        .context("failed to launch `git status`")?;
    if !output.status.success() {
        bail!(
            "`git status` failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(parse_porcelain(&String::from_utf8_lossy(&output.stdout)))
}

fn parse_porcelain(out: &str) -> VcsState {
    let mut staged = Vec::new();
    let mut dirty = Vec::new();

    for line in out.lines() {
        let (Some(x), Some(y), Some(path)) =
            (line.chars().next(), line.chars().nth(1), line.get(3..))
        else {
            continue;
        };
        if x != ' ' && x != '?' {
            staged.push(path.to_owned());
        }
        if y != ' ' {
            dirty.push(path.to_owned());
        }
    }

    if staged.is_empty() && dirty.is_empty() {
        VcsState::Clean
    } else {
        VcsState::Changed { staged, dirty }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn porcelain() {
        assert_eq!(parse_porcelain(""), VcsState::Clean);
        assert_eq!(
            parse_porcelain("M  src/a.rs\n M src/b.rs\nMM src/c.rs\n?? new.rs\n"),
            VcsState::Changed {
                staged: vec!["src/a.rs".into(), "src/c.rs".into()],
                dirty: vec!["src/b.rs".into(), "src/c.rs".into(), "new.rs".into()],
            }
        );
    }
}
