// apply.rs

use crate::fixes::{Fix, Replacement};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyError {
    OutOfBounds {
        start: usize,
        end: usize,
        len: usize,
    },
    NotCharBoundary {
        offset: usize,
    },
    Stale {
        start: usize,
        end: usize,
        expected: String,
        found: String,
    },
    Overlap {
        first: (usize, usize),
        second: (usize, usize),
    },
}

impl fmt::Display for ApplyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApplyError::OutOfBounds { start, end, len } => {
                write!(
                    f,
                    "range {start}..{end} is out of bounds (file has {len} bytes)"
                )
            }
            ApplyError::NotCharBoundary { offset } => {
                write!(f, "byte offset {offset} is not on a UTF-8 char boundary")
            }
            ApplyError::Stale {
                start,
                end,
                expected,
                found,
            } => write!(
                f,
                "stale offsets at {start}..{end}: expected {expected:?}, found {found:?} (file changed after clippy ran?)"
            ),
            ApplyError::Overlap { first, second } => write!(
                f,
                "replacements {}..{} and {}..{} overlap",
                first.0, first.1, second.0, second.1
            ),
        }
    }
}

impl std::error::Error for ApplyError {}

pub fn validate(source: &str, r: &Replacement) -> Result<(), ApplyError> {
    let (start, end) = (r.byte_start, r.byte_end);
    if start > end || end > source.len() {
        return Err(ApplyError::OutOfBounds {
            start,
            end,
            len: source.len(),
        });
    }
    for offset in [start, end] {
        if !source.is_char_boundary(offset) {
            return Err(ApplyError::NotCharBoundary { offset });
        }
    }
    if let Some(expected) = &r.original {
        let found = &source[start..end];
        if found != expected {
            return Err(ApplyError::Stale {
                start,
                end,
                expected: expected.clone(),
                found: found.to_owned(),
            });
        }
    }
    Ok(())
}

#[allow(dead_code)]
pub fn apply_one(source: &str, r: &Replacement) -> Result<String, ApplyError> {
    validate(source, r)?;
    let mut out = source.to_owned();
    out.replace_range(r.byte_start..r.byte_end, &r.text);
    Ok(out)
}

pub fn conflicts(a: &Replacement, b: &Replacement) -> bool {
    if a.byte_start == b.byte_start && a.byte_end == b.byte_end && a.text == b.text {
        return false;
    }
    match (a.is_insertion(), b.is_insertion()) {
        // two different insertions at the same point
        (true, true) => a.byte_start == b.byte_start,
        // insertion strictly inside a range
        (true, false) => b.byte_start < a.byte_start && a.byte_start < b.byte_end,
        (false, true) => a.byte_start < b.byte_start && b.byte_start < a.byte_end,
        // half-open ranges
        (false, false) => a.byte_start < b.byte_end && b.byte_start < a.byte_end,
    }
}

// a fix whose own replacements overlap is a lint bug, the whole fix must be dropped
pub fn check_fix(fix: &Fix) -> Result<(), ApplyError> {
    for (i, a) in fix.replacements.iter().enumerate() {
        for b in &fix.replacements[i + 1..] {
            if conflicts(a, b) {
                return Err(ApplyError::Overlap {
                    first: (a.byte_start, a.byte_end),
                    second: (b.byte_start, b.byte_end),
                });
            }
        }
    }
    Ok(())
}

// all or nothing&  anyone invalid - nothing applied
pub fn apply_replacements(
    source: &str,
    replacements: &[&Replacement],
) -> Result<String, ApplyError> {
    let mut sorted: Vec<&Replacement> = replacements.to_vec();
    sorted.sort_by(|a, b| {
        b.byte_start
            .cmp(&a.byte_start)
            .then(b.byte_end.cmp(&a.byte_end))
    });
    sorted.dedup_by(|a, b| {
        a.byte_start == b.byte_start && a.byte_end == b.byte_end && a.text == b.text
    });

    for r in &sorted {
        validate(source, r)?;
    }

    for (i, a) in sorted.iter().enumerate() {
        for b in &sorted[i + 1..] {
            if conflicts(a, b) {
                return Err(ApplyError::Overlap {
                    first: (b.byte_start, b.byte_end),
                    second: (a.byte_start, a.byte_end),
                });
            }
        }
    }

    let mut out = source.to_owned();
    for r in sorted {
        out.replace_range(r.byte_start..r.byte_end, &r.text);
    }
    Ok(out)
}

// applies non-conflicting fixes of one file
pub fn apply_all(source: &str, fixes: &[Fix]) -> Result<String, ApplyError> {
    for fix in fixes {
        check_fix(fix)?;
    }
    let all: Vec<&Replacement> = fixes.iter().flat_map(|f| &f.replacements).collect();
    apply_replacements(source, &all)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::Applicability;
    use std::path::PathBuf;

    fn r(start: usize, end: usize, text: &str) -> Replacement {
        Replacement {
            byte_start: start,
            byte_end: end,
            text: text.into(),
            original: None,
        }
    }

    fn fix(replacements: Vec<Replacement>) -> Fix {
        Fix {
            file: PathBuf::from("src/lib.rs"),
            lint: None,
            applicability: Applicability::MachineApplicable,
            message: String::new(),
            line: 1,
            replacements,
        }
    }

    #[test]
    fn replaces_inserts_deletes() {
        assert_eq!(
            apply_one("let a = 1;", &r(4, 5, "_a")).unwrap(),
            "let _a = 1;"
        );
        assert_eq!(
            apply_one("let a = 1;", &r(4, 4, "mut ")).unwrap(),
            "let mut a = 1;"
        );
        assert_eq!(
            apply_one("use std::io;\nfn main() {}", &r(0, 13, "")).unwrap(),
            "fn main() {}"
        );
    }

    #[test]
    fn byte_offsets_after_cyrillic() {
        let src = "let s = \"привет\"; let a = 1;";
        let start = src.find("a =").unwrap();
        assert_eq!(start, 28);
        let out = apply_one(src, &r(start, start + 1, "_a")).unwrap();
        assert_eq!(out, "let s = \"привет\"; let _a = 1;");
    }

    #[test]
    fn rejects_bad_ranges() {
        let src = "привет";
        assert_eq!(
            apply_one(src, &r(1, 2, "x")),
            Err(ApplyError::NotCharBoundary { offset: 1 })
        );
        assert!(matches!(
            apply_one(src, &r(0, 100, "x")),
            Err(ApplyError::OutOfBounds { .. })
        ));
        assert!(matches!(
            apply_one(src, &r(4, 2, "x")),
            Err(ApplyError::OutOfBounds { .. })
        ));
    }

    #[test]
    fn detects_stale_offsets() {
        let mut rep = r(4, 5, "_a");
        rep.original = Some("b".into());
        assert!(matches!(
            apply_one("let a = 1;", &rep),
            Err(ApplyError::Stale { .. })
        ));
        rep.original = Some("a".into());
        assert!(apply_one("let a = 1;", &rep).is_ok());
    }

    #[test]
    fn applies_many_from_the_end() {
        let src = "let a = 1; let b = 2; let c = 3;";
        let fixes = vec![
            fix(vec![r(4, 5, "_a")]),
            fix(vec![r(26, 27, "_c")]),
            fix(vec![r(15, 16, "_bbbbb")]),
        ];
        assert_eq!(
            apply_all(src, &fixes).unwrap(),
            "let _a = 1; let _bbbbb = 2; let _c = 3;"
        );
    }

    #[test]
    fn multi_part_fix_is_atomic() {
        let src = "x.len() == 0";
        let f = fix(vec![r(0, 0, "!"), r(1, 12, ".is_empty()")]);
        assert_eq!(apply_all(src, &[f]).unwrap(), "!x.is_empty()");

        let f = fix(vec![r(0, 0, "!"), r(1, 100, ".is_empty()")]);
        assert!(apply_all(src, &[f]).is_err());
    }

    #[test]
    fn self_overlapping_fix_is_rejected() {
        let f = fix(vec![r(0, 5, "a"), r(3, 8, "b")]);
        assert!(matches!(check_fix(&f), Err(ApplyError::Overlap { .. })));
    }

    #[test]
    fn insertion_at_start_of_replaced_range() {
        let out = apply_replacements("abc", &[&r(0, 0, "X"), &r(0, 1, "Y")]).unwrap();
        assert_eq!(out, "XYbc");
        let out = apply_replacements("abc", &[&r(1, 1, "X"), &r(0, 1, "Y")]).unwrap();
        assert_eq!(out, "YXbc");
    }

    #[test]
    fn conflict_rules() {
        assert!(!conflicts(&r(0, 2, "a"), &r(2, 4, "b")));
        assert!(conflicts(&r(0, 3, "a"), &r(2, 4, "b")));
        assert!(conflicts(&r(0, 10, "a"), &r(2, 4, "b")));
        assert!(!conflicts(&r(0, 3, "a"), &r(0, 3, "a")));
        assert!(conflicts(&r(0, 3, "a"), &r(0, 3, "b")));
        assert!(conflicts(&r(2, 2, "a"), &r(2, 2, "b")));
        assert!(!conflicts(&r(2, 2, "a"), &r(2, 5, "b")));
        assert!(!conflicts(&r(5, 5, "a"), &r(2, 5, "b")));
        assert!(conflicts(&r(3, 3, "a"), &r(2, 5, "b")));
    }
}
