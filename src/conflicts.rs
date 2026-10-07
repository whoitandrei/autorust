// conflicts.rs

use crate::apply::{check_fix, conflicts};
use crate::fixes::{Fix, Replacement};

use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeferReason {
    SelfOverlap,
    ConflictsWith { lint: Option<String>, line: usize },
}

#[derive(Debug, Clone)]
pub struct Deferred {
    pub fix: Fix,
    pub reason: DeferReason,
}

#[derive(Debug, Default)]
pub struct FileFixes {
    pub accepted: Vec<Fix>,
    pub deferred: Vec<Deferred>,
}

fn order_key(fix: &Fix) -> (usize, usize, Option<String>, Vec<String>) {
    let start = fix
        .replacements
        .iter()
        .map(|r| r.byte_start)
        .min()
        .unwrap_or(0);
    let end = fix
        .replacements
        .iter()
        .map(|r| r.byte_end)
        .max()
        .unwrap_or(0);
    let texts = fix.replacements.iter().map(|r| r.text.clone()).collect();
    (start, end, fix.lint.clone(), texts)
}

// resolves conflicts of fixes that belong to one file
pub fn resolve_file(mut fixes: Vec<Fix>) -> FileFixes {
    fixes.sort_by_key(order_key);

    let mut out = FileFixes::default();
    let mut taken: Vec<(Replacement, usize)> = Vec::new();

    for fix in fixes {
        if check_fix(&fix).is_err() {
            out.deferred.push(Deferred {
                fix,
                reason: DeferReason::SelfOverlap,
            });
            continue;
        }

        let clash = fix
            .replacements
            .iter()
            .find_map(|r| taken.iter().find(|(t, _)| conflicts(r, t)).map(|(_, i)| *i));

        match clash {
            Some(i) => {
                let other = &out.accepted[i];
                let reason = DeferReason::ConflictsWith {
                    lint: other.lint.clone(),
                    line: other.line,
                };
                out.deferred.push(Deferred { fix, reason });
            }
            None => {
                let idx = out.accepted.len();
                taken.extend(fix.replacements.iter().map(|r| (r.clone(), idx)));
                out.accepted.push(fix);
            }
        }
    }

    out
}

// groups fixes by file and resolves each group
pub fn resolve(fixes: Vec<Fix>) -> BTreeMap<PathBuf, FileFixes> {
    let mut by_file: BTreeMap<PathBuf, Vec<Fix>> = BTreeMap::new();
    for fix in fixes {
        by_file.entry(fix.file.clone()).or_default().push(fix);
    }
    by_file
        .into_iter()
        .map(|(file, fixes)| (file, resolve_file(fixes)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apply::apply_all;
    use crate::diagnostics::Applicability;

    fn fix(lint: &str, reps: &[(usize, usize, &str)]) -> Fix {
        Fix {
            file: PathBuf::from("src/lib.rs"),
            lint: Some(lint.into()),
            applicability: Applicability::MachineApplicable,
            message: String::new(),
            line: 1,
            replacements: reps
                .iter()
                .map(|&(s, e, t)| Replacement {
                    byte_start: s,
                    byte_end: e,
                    text: t.into(),
                    original: None,
                })
                .collect(),
        }
    }

    #[test]
    fn disjoint_fixes_all_accepted() {
        let res = resolve_file(vec![fix("a", &[(10, 12, "x")]), fix("b", &[(0, 2, "y")])]);
        assert_eq!(res.accepted.len(), 2);
        assert!(res.deferred.is_empty());
        assert_eq!(res.accepted[0].lint.as_deref(), Some("b"));
    }

    #[test]
    fn duplicate_replacement_in_different_fixes() {
        let res = resolve_file(vec![
            fix("a", &[(0, 2, "x")]),
            fix("b", &[(0, 2, "x"), (5, 5, "!")]),
        ]);
        assert_eq!(res.accepted.len(), 2);
        let out = apply_all("0123456789", &res.accepted).unwrap();
        assert_eq!(out, "x234!56789");
    }

    #[test]
    fn nested_and_partial_overlaps_are_deferred() {
        let res = resolve_file(vec![
            fix("outer", &[(0, 10, "x")]),
            fix("inner", &[(2, 4, "y")]),
            fix("partial", &[(8, 12, "z")]),
        ]);
        assert_eq!(res.accepted.len(), 1);
        assert_eq!(res.accepted[0].lint.as_deref(), Some("outer"));
        assert_eq!(res.deferred.len(), 2);
        assert_eq!(
            res.deferred[0].reason,
            DeferReason::ConflictsWith {
                lint: Some("outer".into()),
                line: 1
            }
        );
    }

    #[test]
    fn insertions() {
        let res = resolve_file(vec![
            fix("a", &[(5, 5, "x")]),
            fix("b", &[(5, 5, "y")]),
            fix("c", &[(2, 5, "z")]),
        ]);
        let lints: Vec<_> = res
            .accepted
            .iter()
            .map(|f| f.lint.clone().unwrap())
            .collect();
        assert_eq!(lints, ["c", "a"]);
        assert_eq!(res.deferred.len(), 1);
    }

    #[test]
    fn self_overlap() {
        let res = resolve_file(vec![fix("bad", &[(0, 5, "x"), (3, 6, "y")])]);
        assert!(res.accepted.is_empty());
        assert_eq!(res.deferred[0].reason, DeferReason::SelfOverlap);
    }

    #[test]
    fn deterministic_regardless_of_input_order() {
        let a = fix("a", &[(0, 4, "x")]);
        let b = fix("b", &[(2, 6, "y")]);
        let r1 = resolve_file(vec![a.clone(), b.clone()]);
        let r2 = resolve_file(vec![b, a]);
        assert_eq!(r1.accepted, r2.accepted);
    }
}
