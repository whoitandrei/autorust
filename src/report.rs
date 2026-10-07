// report.rs

use crate::conflicts::{DeferReason, Deferred};
use crate::fixes::Fix;

use similar::TextDiff;
use std::path::Path;

#[derive(Debug, Clone, Copy)]
pub struct Style {
    pub color: bool,
}

impl Style {
    fn paint(&self, code: &str, text: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_owned()
        }
    }
    pub fn red(&self, t: &str) -> String {
        self.paint("31", t)
    }
    pub fn green(&self, t: &str) -> String {
        self.paint("32", t)
    }
    pub fn yellow(&self, t: &str) -> String {
        self.paint("33", t)
    }
    pub fn cyan(&self, t: &str) -> String {
        self.paint("36", t)
    }
    pub fn bold(&self, t: &str) -> String {
        self.paint("1", t)
    }
}

pub fn display_path(file: &Path, root: &Path) -> String {
    file.strip_prefix(root)
        .unwrap_or(file)
        .display()
        .to_string()
}

// чтобы выглядело красиво
pub fn unified_diff(old: &str, new: &str, path: &str) -> String {
    TextDiff::from_lines(old, new)
        .unified_diff()
        .context_radius(3)
        .header(&format!("a/{path}"), &format!("b/{path}"))
        .to_string()
}

pub fn colorize_diff(diff: &str, style: Style) -> String {
    if !style.color {
        return diff.to_owned();
    }
    let mut out = String::with_capacity(diff.len());
    for line in diff.split_inclusive('\n') {
        let (body, nl) = match line.strip_suffix('\n') {
            Some(b) => (b, "\n"),
            None => (line, ""),
        };
        let painted = if body.starts_with("+++") || body.starts_with("---") {
            style.bold(body)
        } else if body.starts_with("@@") {
            style.cyan(body)
        } else if body.starts_with('+') {
            style.green(body)
        } else if body.starts_with('-') {
            style.red(body)
        } else {
            body.to_owned()
        };
        out.push_str(&painted);
        out.push_str(nl);
    }
    out
}

pub fn fix_line(fix: &Fix, path: &str) -> String {
    format!(
        "{path}:{}: [{}] {}",
        fix.line,
        fix.lint.as_deref().unwrap_or("-"),
        fix.message
    )
}

pub fn deferred_line(d: &Deferred, path: &str) -> String {
    let why = match &d.reason {
        DeferReason::SelfOverlap => "replacements of this fix overlap each other".to_owned(),
        DeferReason::ConflictsWith { lint, line } => format!(
            "conflicts with [{}] at line {line}, will be picked up by the next run",
            lint.as_deref().unwrap_or("-")
        ),
    };
    format!("{} ({why})", fix_line(&d.fix, path))
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    pub files: usize,
    pub applied: usize,
    pub deferred: usize,
    pub failed_files: usize,
}

impl Stats {
    pub fn add(&mut self, other: Stats) {
        self.files += other.files;
        self.applied += other.applied;
        self.deferred += other.deferred;
        self.failed_files += other.failed_files;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_has_headers_and_changes() {
        let diff = unified_diff("a\nb\nc\n", "a\nB\nc\n", "src/lib.rs");
        assert_eq!(
            diff,
            "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n"
        );
    }

    #[test]
    fn no_color_is_identity() {
        let d = "--- a\n+++ b\n-x\n+y\n";
        assert_eq!(colorize_diff(d, Style { color: false }), d);
        assert!(colorize_diff(d, Style { color: true }).contains("\x1b[32m+y\x1b[0m\n"));
    }
}
