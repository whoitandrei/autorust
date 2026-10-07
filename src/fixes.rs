// fixes.rs

use crate::diagnostics::{Applicability, CompilerMessage, Diagnostic, DiagnosticSpan};

use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Replacement {
    pub byte_start: usize,
    pub byte_end: usize,
    pub text: String,
    // text clippy saw in this range
    pub original: Option<String>,
}

impl Replacement {
    pub fn is_insertion(&self) -> bool {
        self.byte_start == self.byte_end
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fix {
    // absolute path
    pub file: PathBuf,
    pub lint: Option<String>,
    pub applicability: Applicability,
    pub message: String,
    // line of the first replacement (1-based)
    pub line: usize,
    pub replacements: Vec<Replacement>, // atomic!!
}

// which fixes we are going to apply
#[derive(Debug, Clone)]
pub struct FixFilter {
    // canonical root of the workspace, fixes outside of it are dropped
    pub root: PathBuf,
    pub allow_maybe_incorrect: bool,
    // if not empty, only these lints
    pub lints: Vec<String>,
    pub exclude_lints: Vec<String>,
}

impl FixFilter {
    pub fn new(root: PathBuf) -> Self {
        FixFilter {
            root,
            allow_maybe_incorrect: false,
            lints: Vec::new(),
            exclude_lints: Vec::new(),
        }
    }

    fn applicability_ok(&self, a: Applicability) -> bool {
        match a {
            Applicability::MachineApplicable => true,
            Applicability::MaybeIncorrect => self.allow_maybe_incorrect,
            // never be applied blindly
            _ => false,
        }
    }

    fn lint_ok(&self, lint: Option<&str>) -> bool {
        let matches = |names: &[String]| match lint {
            Some(l) => names.iter().any(|n| lint_matches(n, l)),
            None => false,
        };

        if !self.lints.is_empty() && !matches(&self.lints) {
            return false;
        }
        !matches(&self.exclude_lints)
    }
}

fn lint_matches(pattern: &str, lint: &str) -> bool {
    let pattern = pattern.replace('-', "_");
    lint == pattern || lint.strip_prefix("clippy::") == Some(pattern.as_str())
}

// columns are 1-based and counted in chars
fn span_original_text(span: &DiagnosticSpan) -> Option<String> {
    if span.text.is_empty() {
        return None;
    }

    let mut parts = Vec::with_capacity(span.text.len());
    for line in &span.text {
        let start = line.highlight_start.checked_sub(1)?;
        let end = line.highlight_end.checked_sub(1)?;
        let part: String = line
            .text
            .chars()
            .skip(start)
            .take(end.checked_sub(start)?)
            .collect();
        parts.push(part);
    }
    Some(parts.join("\n"))
}

// worst applicability of all spans wins
fn combine(a: Applicability, b: Applicability) -> Applicability {
    fn rank(a: Applicability) -> u8 {
        match a {
            Applicability::MachineApplicable => 0,
            Applicability::MaybeIncorrect => 1,
            Applicability::HasPlaceholders => 2,
            Applicability::Unspecified => 3,
            Applicability::Unknown => 4,
        }
    }
    if rank(a) >= rank(b) { a } else { b }
}

fn resolve_path(root: &Path, file_name: &Path) -> Option<PathBuf> {
    let path = if file_name.is_absolute() {
        file_name.to_path_buf()
    } else {
        root.join(file_name)
    };
    let canonical = path.canonicalize().ok()?;
    canonical.starts_with(root).then_some(canonical)
}

// one diagnostic node -> at most one atomic fix made of all its suggested spans
fn diagnostic_to_fix(
    diagnostic: &Diagnostic,
    lint: Option<&str>,
    filter: &FixFilter,
) -> Option<Fix> {
    let spans: Vec<&DiagnosticSpan> = diagnostic
        .spans
        .iter()
        .filter(|s| s.suggested_replacement.is_some())
        .collect();

    if spans.is_empty() {
        return None;
    }

    let file_name = &spans[0].file_name;
    if spans.iter().any(|s| &s.file_name != file_name) {
        return None;
    }

    if spans.iter().any(|s| s.expansion.is_some()) {
        return None;
    }

    let applicability = spans
        .iter()
        .map(|s| {
            s.suggestion_applicability
                .unwrap_or(Applicability::Unspecified)
        })
        .fold(Applicability::MachineApplicable, combine);

    if !filter.applicability_ok(applicability) || !filter.lint_ok(lint) {
        return None;
    }

    let file = resolve_path(&filter.root, file_name)?;

    let replacements = spans
        .iter()
        .map(|s| Replacement {
            byte_start: s.byte_start,
            byte_end: s.byte_end,
            text: s.suggested_replacement.clone().unwrap_or_default(),
            original: span_original_text(s),
        })
        .collect();

    Some(Fix {
        file,
        lint: lint.map(str::to_owned),
        applicability,
        message: diagnostic.message.clone(),
        line: spans.iter().map(|s| s.line_start).min().unwrap_or(0),
        replacements,
    })
}

fn collect_diagnostic_fixes(
    diagnostic: &Diagnostic,
    lint: Option<&str>,
    filter: &FixFilter,
    fixes: &mut Vec<Fix>,
) {
    let lint = diagnostic.code.as_ref().map(|c| c.code.as_str()).or(lint);

    if let Some(fix) = diagnostic_to_fix(diagnostic, lint, filter) {
        fixes.push(fix);
    }

    for child in &diagnostic.children {
        collect_diagnostic_fixes(child, lint, filter, fixes);
    }
}

pub fn collect_fixes(messages: &[CompilerMessage], filter: &FixFilter) -> Vec<Fix> {
    let mut fixes = Vec::new();

    for message in messages {
        collect_diagnostic_fixes(&message.diagnostic, None, filter, &mut fixes);
    }

    dedup(fixes)
}

fn dedup(fixes: Vec<Fix>) -> Vec<Fix> {
    let mut seen = HashSet::new();
    fixes
        .into_iter()
        .filter(|f| seen.insert((f.file.clone(), f.replacements.clone())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::parse;

    fn line(file: &str, applicability: &str, expansion: &str) -> String {
        format!(
            r#"{{"reason":"compiler-message","target":{{"kind":["lib"],"name":"t","src_path":"src/lib.rs"}},"message":{{"message":"unused variable: `a`","code":{{"code":"unused_variables"}},"level":"warning","rendered":null,"spans":[],"children":[{{"message":"prefix it","code":null,"level":"help","rendered":null,"children":[],"spans":[{{"file_name":"{file}","byte_start":8,"byte_end":9,"line_start":2,"line_end":2,"column_start":9,"column_end":10,"is_primary":true,"label":null,"suggested_replacement":"_a","suggestion_applicability":"{applicability}","expansion":{expansion},"text":[{{"text":"    let a = 1;","highlight_start":9,"highlight_end":10}}]}}]}}]}}}}"#
        )
    }

    fn setup() -> (tempfile::TempDir, FixFilter) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(
            dir.path().join("src/lib.rs"),
            "fn f() {\n    let a = 1;\n}\n",
        )
        .unwrap();
        let root = dir.path().canonicalize().unwrap();
        (dir, FixFilter::new(root))
    }

    #[test]
    fn extracts_fix_from_children_with_parent_lint() {
        let (_dir, filter) = setup();
        let outcome = parse(&line("src/lib.rs", "MachineApplicable", "null"));
        let fixes = collect_fixes(&outcome.messages, &filter);

        assert_eq!(fixes.len(), 1);
        let fix = &fixes[0];
        assert_eq!(fix.lint.as_deref(), Some("unused_variables"));
        assert_eq!(fix.line, 2);
        assert_eq!(
            fix.replacements,
            vec![Replacement {
                byte_start: 8,
                byte_end: 9,
                text: "_a".into(),
                original: Some("a".into()),
            }]
        );
    }

    #[test]
    fn deduplicates_targets() {
        let (_dir, filter) = setup();
        let l = line("src/lib.rs", "MachineApplicable", "null");
        let outcome = parse(&format!("{l}\n{l}\n"));
        assert_eq!(collect_fixes(&outcome.messages, &filter).len(), 1);
    }

    #[test]
    fn filters() {
        let (_dir, mut filter) = setup();

        let maybe = parse(&line("src/lib.rs", "MaybeIncorrect", "null"));
        assert!(collect_fixes(&maybe.messages, &filter).is_empty());
        filter.allow_maybe_incorrect = true;
        assert_eq!(collect_fixes(&maybe.messages, &filter).len(), 1);

        let placeholders = parse(&line("src/lib.rs", "HasPlaceholders", "null"));
        assert!(collect_fixes(&placeholders.messages, &filter).is_empty());

        let in_macro = parse(&line(
            "src/lib.rs",
            "MachineApplicable",
            r#"{"macro_decl_name":"m!"}"#,
        ));
        assert!(collect_fixes(&in_macro.messages, &filter).is_empty());

        let outside = parse(&line("/etc/hosts", "MachineApplicable", "null"));
        assert!(collect_fixes(&outside.messages, &filter).is_empty());

        let ok = parse(&line("src/lib.rs", "MachineApplicable", "null"));
        filter.exclude_lints = vec!["unused_variables".into()];
        assert!(collect_fixes(&ok.messages, &filter).is_empty());
        filter.exclude_lints.clear();
        filter.lints = vec!["needless_return".into()];
        assert!(collect_fixes(&ok.messages, &filter).is_empty());
    }

    #[test]
    fn lint_name_matching() {
        assert!(lint_matches("needless_return", "clippy::needless_return"));
        assert!(lint_matches("needless-return", "clippy::needless_return"));
        assert!(lint_matches(
            "clippy::needless_return",
            "clippy::needless_return"
        ));
        assert!(lint_matches("unused_imports", "unused_imports"));
        assert!(!lint_matches("needless", "clippy::needless_return"));
    }

    #[test]
    fn original_text_spans_lines_and_counts_chars() {
        let outcome = parse(&line("src/lib.rs", "MachineApplicable", "null"));
        let span = &outcome.messages[0].diagnostic.children[0].spans[0];
        assert_eq!(span_original_text(span).as_deref(), Some("a"));

        let mut span = span.clone();
        span.text = vec![crate::diagnostics::SpanLine {
            text: "let s = \"привет\"; x".into(),
            highlight_start: 19,
            highlight_end: 20,
        }];
        assert_eq!(span_original_text(&span).as_deref(), Some("x"));
    }
}
