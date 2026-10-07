// main.rs

use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

mod apply;
mod cli;
mod conflicts;
mod diagnostics;
mod fixes;
mod report;
mod runner;

use cli::{Cli, ColorChoice, Level};
use conflicts::FileFixes;
use fixes::{Fix, FixFilter};
use report::{Stats, Style};
use runner::VcsState;

// nothing to fix or everything applied
const EXIT_OK: u8 = 0;
// some fixes are left unapplied
const EXIT_PENDING: u8 = 1;
// autorust failed!! hell nah mamochki oh no!
const EXIT_ERROR: u8 = 2;

fn main() -> ExitCode {
    let cli = Cli::parse_args();
    match run(&cli) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::from(EXIT_ERROR)
        }
    }
}

fn style_for(choice: ColorChoice, is_tty: bool) -> Style {
    let color = match choice {
        ColorChoice::Always => true,
        ColorChoice::Never => false,
        ColorChoice::Auto => is_tty && std::env::var_os("NO_COLOR").is_none(),
    };
    Style { color }
}

struct Ctx<'a> {
    cli: &'a Cli,
    manifest: PathBuf,
    root: PathBuf,
    filter: FixFilter,
    out: Style,
    err: Style,
}

fn run(cli: &Cli) -> Result<u8> {
    let manifest = runner::manifest_file(&cli.manifest_path)?;
    let root = runner::workspace_root(&manifest)?;

    if cli.writes() {
        ensure_vcs_ok(cli, &root)?;
    }

    let mut filter = FixFilter::new(root.clone());
    filter.allow_maybe_incorrect = cli.applicability == Level::MaybeIncorrect;
    filter.lints = cli.lints.clone();
    filter.exclude_lints = cli.exclude_lints.clone();

    let ctx = Ctx {
        cli,
        manifest,
        root,
        filter,
        out: style_for(cli.color, std::io::stdout().is_terminal()),
        err: style_for(cli.color, std::io::stderr().is_terminal()),
    };

    let passes = if cli.writes() { cli.passes } else { 1 };
    let mut total = Stats::default();
    let mut last = Stats::default();
    let mut originals: BTreeMap<PathBuf, String> = BTreeMap::new();

    for pass in 1..=passes {
        let fixes = collect(&ctx)?;
        if fixes.is_empty() {
            last = Stats::default();
            break;
        }
        if passes > 1 {
            eprintln!(
                "{}",
                ctx.err
                    .bold(&format!("pass {pass}: {} fix(es) found", fixes.len()))
            );
        }

        let resolved = conflicts::resolve(fixes);
        last = process(&ctx, resolved, &mut originals)?;
        total.add(last);

        if cli.writes() && cli.verify && last.files > 0 {
            verify_or_rollback(&ctx, &originals)?;
        }
        if last.files == 0 {
            // nothing to applied in this pass
            break;
        }
    }

    print_summary(&ctx, total, last);

    let pending = last.deferred + last.failed_files;
    let code = if cli.check {
        if total.applied + pending > 0 {
            EXIT_PENDING
        } else {
            EXIT_OK
        }
    } else if pending > 0 {
        EXIT_PENDING
    } else {
        EXIT_OK
    };
    Ok(code)
}

// do not overwrite work that is not committed
// не перезаписывать то что не закоммитили
fn ensure_vcs_ok(cli: &Cli, root: &Path) -> Result<()> {
    match runner::vcs_state(root)? {
        VcsState::NoVcs if !cli.allow_no_vcs => bail!(
            "no VCS found for {}, autorust could overwrite your code without a way back\n\
             pass --allow-no-vcs to proceed anyway",
            root.display()
        ),
        VcsState::Changed { staged, dirty } if !cli.allow_dirty => {
            let mut problems: Vec<String> =
                dirty.iter().map(|p| format!("  * {p} (dirty)")).collect();
            if !cli.allow_staged {
                problems.extend(staged.iter().map(|p| format!("  * {p} (staged)")));
            }
            if !problems.is_empty() {
                bail!(
                    "the working directory has uncommitted changes:\n{}\n\
                     commit them first, or pass --allow-dirty / --allow-staged",
                    problems.join("\n")
                );
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

// rustc errors means code does not compile and we can not handle this
// ошибки компиляции -> абортимся
fn is_hard_error(d: &diagnostics::Diagnostic) -> bool {
    if d.level != "error" && d.level != "error: internal compiler error" {
        return false;
    }
    match &d.code {
        Some(c) => c.code.starts_with('E') && c.code[1..].chars().all(|ch| ch.is_ascii_digit()),
        None => !d.message.starts_with("aborting due to"),
    }
}

fn collect(ctx: &Ctx) -> Result<Vec<Fix>> {
    let run = runner::run_clippy(&ctx.manifest, &ctx.cli.cargo_args)?;
    let outcome = diagnostics::parse(&run.lines);

    if !outcome.errors.is_empty() {
        eprintln!(
            "{} skipped {} line(s) of cargo output that are not valid JSON (first: line {}: {})",
            ctx.err.yellow("warning:"),
            outcome.errors.len(),
            outcome.errors[0].line_no,
            outcome.errors[0].message
        );
    }

    if !run.exit_status.success() {
        let hard: Vec<_> = outcome
            .messages
            .iter()
            .map(|m| &m.diagnostic)
            .filter(|d| is_hard_error(d))
            .collect();

        if let Some(first) = hard.first() {
            let rendered = first.rendered.as_deref().unwrap_or(&first.message);
            bail!(
                "the crate does not compile ({} error(s)), fix it first:\n{rendered}",
                hard.len()
            );
        }
        if outcome.messages.is_empty() {
            bail!(
                "`cargo clippy` failed ({}):\n{}",
                run.exit_status,
                run.err.trim_end()
            );
        }
    }

    Ok(fixes::collect_fixes(&outcome.messages, &ctx.filter))
}

fn process(
    ctx: &Ctx,
    resolved: BTreeMap<PathBuf, FileFixes>,
    originals: &mut BTreeMap<PathBuf, String>,
) -> Result<Stats> {
    let mut stats = Stats::default();

    for (file, file_fixes) in resolved {
        let path = report::display_path(&file, &ctx.root);

        stats.deferred += file_fixes.deferred.len();
        for d in &file_fixes.deferred {
            eprintln!(
                "{} {}",
                ctx.err.yellow("deferred:"),
                report::deferred_line(d, &path)
            );
        }
        if file_fixes.accepted.is_empty() {
            continue;
        }

        let source = std::fs::read_to_string(&file)
            .with_context(|| format!("can't read {}", file.display()))?;

        let fixed = match apply::apply_all(&source, &file_fixes.accepted) {
            Ok(fixed) => fixed,
            Err(e) => {
                // whole file is skipped
                eprintln!("{} {path}: {e}", ctx.err.red("skipped:"));
                stats.failed_files += 1;
                continue;
            }
        };
        if fixed == source {
            continue;
        }

        let verb = if ctx.cli.writes() {
            "fixed:"
        } else {
            "fixable:"
        };
        for fix in &file_fixes.accepted {
            eprintln!("{} {}", ctx.err.green(verb), report::fix_line(fix, &path));
        }
        stats.files += 1;
        stats.applied += file_fixes.accepted.len();

        if ctx.cli.dry_run {
            let diff = report::unified_diff(&source, &fixed, &path);
            print!("{}", report::colorize_diff(&diff, ctx.out));
        }
        if ctx.cli.writes() {
            write_atomic(&file, &fixed)?;
            originals.entry(file).or_insert(source);
        }
    }

    Ok(stats)
}

// write to a temp file next to the target and rename
fn write_atomic(file: &Path, contents: &str) -> Result<()> {
    let name = file
        .file_name()
        .context("file without a name")?
        .to_string_lossy();
    let tmp = file.with_file_name(format!(".{name}.autorust-tmp"));

    std::fs::write(&tmp, contents).with_context(|| format!("can't write {}", tmp.display()))?;
    if let Ok(meta) = std::fs::metadata(file) {
        let _ = std::fs::set_permissions(&tmp, meta.permissions());
    }
    std::fs::rename(&tmp, file).with_context(|| {
        let _ = std::fs::remove_file(&tmp);
        format!("can't replace {}", file.display())
    })
}

fn verify_or_rollback(ctx: &Ctx, originals: &BTreeMap<PathBuf, String>) -> Result<()> {
    let Some(errors) = runner::check_builds(&ctx.manifest, &ctx.cli.cargo_args)? else {
        return Ok(());
    };
    for (file, source) in originals {
        write_atomic(file, source)?;
    }
    bail!(
        "the fixes broke the build, {} file(s) restored. `cargo check` said:\n{}",
        originals.len(),
        errors.trim_end()
    )
}

fn print_summary(ctx: &Ctx, total: Stats, last: Stats) {
    let action = if ctx.cli.writes() {
        "applied"
    } else {
        "can be applied"
    };
    let mut line = format!("{} fix(es) {action}", total.applied);
    if last.deferred > 0 {
        line.push_str(&format!(", {} deferred", last.deferred));
    }
    if last.failed_files > 0 {
        line.push_str(&format!(", {} file(s) skipped", last.failed_files));
    }
    if total.applied == 0 && last.deferred == 0 && last.failed_files == 0 {
        line = "nothing to fix".to_owned();
    }
    eprintln!("{}", ctx.err.bold(&line));
}
