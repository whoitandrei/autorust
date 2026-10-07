// diagnostics.rs

use serde::Deserialize;
use std::path::PathBuf;

// struct-helper
#[derive(Debug, Deserialize)]
#[serde(tag = "reason", rename_all = "kebab-case")]
enum CargoMessage {
    // parse compiler message by reason field
    CompilerMessage(Box<CompilerMessage>),

    // do not throw away other strings for compiler updates
    #[serde(other)]
    Other,
}

// return datatypes
//
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct CompilerMessage {
    pub target: Target,

    #[serde(rename = "message")]
    pub diagnostic: Diagnostic,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct Target {
    pub kind: Vec<String>,
    pub name: String,
    pub src_path: PathBuf,
}

// diagnostics

#[derive(Debug, Clone, Deserialize)]
pub struct Diagnostic {
    pub message: String,
    pub code: Option<DiagnosticCode>,
    pub level: String,

    #[serde(default)]
    pub spans: Vec<DiagnosticSpan>,

    #[serde(default)]
    pub children: Vec<Diagnostic>,

    pub rendered: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DiagnosticCode {
    pub code: String,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct DiagnosticSpan {
    pub file_name: PathBuf,

    pub byte_start: usize,
    pub byte_end: usize,

    pub line_start: usize,
    pub line_end: usize,
    pub column_start: usize,
    pub column_end: usize,
    pub is_primary: bool,

    #[serde(default)]
    pub text: Vec<SpanLine>,

    pub label: Option<String>,

    pub suggested_replacement: Option<String>,
    pub suggestion_applicability: Option<Applicability>,

    pub expansion: Option<Expansion>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SpanLine {
    pub text: String,
    pub highlight_start: usize,
    pub highlight_end: usize,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct Expansion {
    pub macro_decl_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
pub enum Applicability {
    MachineApplicable,
    MaybeIncorrect,
    HasPlaceholders,
    Unspecified,

    #[serde(other)]
    Unknown,
}

// result

#[derive(Debug, Default)]
pub struct ParseOutcome {
    pub messages: Vec<CompilerMessage>,
    pub ignored: usize,
    pub errors: Vec<ParseError>,
}

#[derive(Debug)]
pub struct ParseError {
    pub line_no: usize,
    pub message: String,
}

pub fn parse(stdout: &str) -> ParseOutcome {
    let mut outcome = ParseOutcome::default();

    for (idx, line) in stdout.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }

        match serde_json::from_str::<CargoMessage>(line) {
            Ok(CargoMessage::CompilerMessage(m)) => {
                outcome.messages.push(*m);
            }
            Ok(CargoMessage::Other) => {
                outcome.ignored += 1;
            }
            Err(e) => {
                outcome.errors.push(ParseError {
                    line_no: idx + 1,
                    message: e.to_string(),
                });
            }
        }
    }

    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    const UNUSED_IMPORT: &str = r#"{"reason":"compiler-message","package_id":"path+file://Users/andrei/prog/rust/testcrate#0.1.0","manifest_path":"/Users/andrei/prog/rust/testcrate/Cargo.toml","target":{"kind":["bin"],"crate_types":["bin"],"name":"testcrate","src_path":"/Users/andrei/prog/rust/testcrate/src/main.rs","edition":"2024","doc":true,"doctest":false,"test":true},"message":{"rendered":"warning: unused import: `std::io`\n","$message_type":"diagnostic","children":[{"children":[],"code":null,"level":"note","message":"`#[warn(unused_imports)]` (part of `#[warn(unused)]`) on by default","rendered":null,"spans":[]},{"children":[],"code":null,"level":"help","message":"remove the whole `use` item","rendered":null,"spans":[{"byte_end":13,"byte_start":0,"column_end":1,"column_start":1,"expansion":null,"file_name":"src/main.rs","is_primary":true,"label":null,"line_end":2,"line_start":1,"suggested_replacement":"","suggestion_applicability":"MachineApplicable","text":[{"highlight_end":13,"highlight_start":1,"text":"use std::io;"},{"highlight_end":1,"highlight_start":1,"text":""}]}]}],"level":"warning","message":"unused import: `std::io`","spans":[{"byte_end":11,"byte_start":4,"column_end":12,"column_start":5,"expansion":null,"file_name":"src/main.rs","is_primary":true,"label":null,"line_end":1,"line_start":1,"suggested_replacement":null,"suggestion_applicability":null,"text":[{"highlight_end":12,"highlight_start":5,"text":"use std::io;"}]}],"code":{"code":"unused_imports","explanation":null}}}"#;

    #[test]
    fn parses_compiler_message() {
        let outcome = parse(UNUSED_IMPORT);
        assert_eq!(outcome.messages.len(), 1);
        assert!(outcome.errors.is_empty());

        let d = &outcome.messages[0].diagnostic;
        assert_eq!(d.level, "warning");
        assert_eq!(d.code.as_ref().unwrap().code, "unused_imports");
        assert_eq!(d.children.len(), 2);

        assert!(d.spans[0].suggested_replacement.is_none());
        let help = &d.children[1];
        assert_eq!(help.level, "help");
        let span = &help.spans[0];
        assert_eq!(span.file_name, PathBuf::from("src/main.rs"));
        assert_eq!((span.byte_start, span.byte_end), (0, 13));
        assert_eq!(span.suggested_replacement.as_deref(), Some(""));
        assert_eq!(
            span.suggestion_applicability,
            Some(Applicability::MachineApplicable)
        );
        assert!(span.expansion.is_none());
    }

    #[test]
    fn counts_other_messages_and_reports_garbage() {
        let input = format!(
            "{}\n{}\nnot json at all\n\n",
            r#"{"reason":"build-finished","success":true}"#, UNUSED_IMPORT
        );
        let outcome = parse(&input);
        assert_eq!(outcome.messages.len(), 1);
        assert_eq!(outcome.ignored, 1);
        assert_eq!(outcome.errors.len(), 1);
        assert_eq!(outcome.errors[0].line_no, 3);
    }

    #[test]
    fn unknown_applicability_is_not_an_error() {
        let a: Applicability = serde_json::from_str(r#""SomethingNew""#).unwrap();
        assert_eq!(a, Applicability::Unknown);
    }
}
