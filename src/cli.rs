// cli.rs

use clap::{Parser, ValueEnum};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "autorust", version, about)]
pub struct Cli {
    // Cargo.toml
    #[arg(long, value_name = "PATH", default_value = ".")]
    pub manifest_path: PathBuf,

    // print a unified diff instead
    #[arg(long)]
    pub dry_run: bool,

    // CI mode: do not write anything, exit with 1 if there is something to fix
    #[arg(long, conflicts_with = "dry_run")]
    pub check: bool,

    // Fix even if the working directory has uncommitted changes
    #[arg(long)]
    pub allow_dirty: bool,

    // Fix even if the working directory has staged changes
    #[arg(long)]
    pub allow_staged: bool,

    // Fix even if the crate is not under git
    #[arg(long)]
    pub allow_no_vcs: bool,

    // Only apply fixes for these lints (`needless_return` or `clippy::needless_return`)
    #[arg(long = "lint", value_name = "NAME")]
    pub lints: Vec<String>,

    // Never apply fixes for these lints
    #[arg(long = "exclude-lint", value_name = "NAME")]
    pub exclude_lints: Vec<String>,

    // Lowest suggestion applicability to accept
    #[arg(long, value_enum, default_value_t = Level::MachineApplicable)]
    pub applicability: Level,

    // Rerun clippy and apply again until nothing is left (at most N passes)
    #[arg(long, value_name = "N", default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..=32))]
    pub passes: u32,

    // Run `cargo check` after writing and restore the files if it fails
    #[arg(long)]
    pub verify: bool,

    // When to use colors
    #[arg(long, value_enum, default_value_t = ColorChoice::Auto)]
    pub color: ColorChoice,

    // Extra arguments for `cargo clippy` (after `--`)
    #[arg(last = true, value_name = "CARGO_ARGS")]
    pub cargo_args: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Level {
    MachineApplicable,
    MaybeIncorrect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ColorChoice {
    Auto,
    Always,
    Never,
}

impl Cli {
    pub fn parse_args() -> Self {
        let mut args: Vec<_> = std::env::args_os().collect();
        if args.get(1).is_some_and(|a| a == "autorust") {
            args.remove(1);
        }
        Cli::parse_from(args)
    }

    pub fn writes(&self) -> bool {
        !self.dry_run && !self.check
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn parses_flags_and_cargo_args() {
        let cli = Cli::parse_from([
            "autorust",
            "--dry-run",
            "--lint",
            "needless_return",
            "--lint",
            "len_zero",
            "--applicability",
            "maybe-incorrect",
            "--",
            "--features",
            "foo",
        ]);
        assert!(cli.dry_run && !cli.writes());
        assert_eq!(cli.lints, ["needless_return", "len_zero"]);
        assert_eq!(cli.applicability, Level::MaybeIncorrect);
        assert_eq!(cli.cargo_args, ["--features", "foo"]);
    }

    #[test]
    fn check_and_dry_run_conflict() {
        assert!(Cli::try_parse_from(["autorust", "--check", "--dry-run"]).is_err());
    }
}
