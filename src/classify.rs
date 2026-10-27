//! C1 classification: (1) a registered main is protected; (2) a linked worktree inside
//! its own main is secondary; (3) a checkout under a secondary root is secondary;
//! (4) everything else is protected. Unclassifiable is protected.

use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Class {
    Protected,
    Secondary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckoutKind {
    Main,
    Linked { main: PathBuf },
    Clone,
}

#[derive(Debug, Clone)]
pub struct CheckoutFacts {
    pub realpath: Result<PathBuf, String>,
    pub kind: CheckoutKind,
    pub git: Result<(), String>,
}

/// Component-wise "strictly below": `/private/tmpfoo` is not below `/private/tmp`.
fn strictly_below(path: &Path, root: &Path) -> bool {
    path != root && path.starts_with(root)
}

pub fn classify(
    facts: &CheckoutFacts,
    registered_mains: &[PathBuf],
    secondary_roots: &[PathBuf],
) -> (Class, String) {
    let path = match (&facts.realpath, &facts.git) {
        (Err(e), _) => return (Class::Protected, format!("unclassifiable: {e}")),
        (_, Err(e)) => return (Class::Protected, format!("unclassifiable: {e}")),
        (Ok(p), Ok(())) => p,
    };
    if registered_mains.iter().any(|m| m == path) {
        return (Class::Protected, "registered main checkout".into());
    }
    if let CheckoutKind::Linked { main } = &facts.kind {
        if strictly_below(path, main) {
            return (
                Class::Secondary,
                format!("linked worktree inside its main {}", main.display()),
            );
        }
    }
    if let Some(root) = secondary_roots.iter().find(|r| strictly_below(path, r)) {
        return (
            Class::Secondary,
            format!("under secondary root {}", root.display()),
        );
    }
    (Class::Protected, "no secondary rule matched".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    fn facts(path: &str, kind: CheckoutKind) -> CheckoutFacts {
        CheckoutFacts {
            realpath: Ok(p(path)),
            kind,
            git: Ok(()),
        }
    }

    #[test]
    fn rules_apply_in_order_and_unknown_is_protected() {
        let mains = [p("/r/main"), p("/private/tmp/runs/shared")];
        let roots = [p("/private/tmp"), p("/var/folders/x/T")];
        let linked = |m: &str| CheckoutKind::Linked { main: p(m) };
        let cases: Vec<(CheckoutFacts, Class)> = vec![
            (facts("/r/main", CheckoutKind::Main), Class::Protected),
            (
                facts("/private/tmp/runs/shared", CheckoutKind::Clone),
                Class::Protected,
            ),
            (
                facts("/r/main/.worktrees/lane1", linked("/r/main")),
                Class::Secondary,
            ),
            (
                facts("/r/main/.claude/worktrees/x", linked("/r/main")),
                Class::Secondary,
            ),
            (facts("/r/sibling-wt", linked("/r/main")), Class::Protected),
            (
                facts("/private/tmp/run7/klynt", CheckoutKind::Clone),
                Class::Secondary,
            ),
            (
                facts("/private/tmp/wt", linked("/r/main")),
                Class::Secondary,
            ),
            (
                facts("/private/tmpfoo/klynt", CheckoutKind::Clone),
                Class::Protected,
            ),
            (facts("/private/tmp", CheckoutKind::Clone), Class::Protected),
            (
                facts("/Users/me/other", CheckoutKind::Clone),
                Class::Protected,
            ),
            (
                CheckoutFacts {
                    realpath: Err("No such file".into()),
                    kind: CheckoutKind::Clone,
                    git: Ok(()),
                },
                Class::Protected,
            ),
            (
                CheckoutFacts {
                    realpath: Ok(p("/private/tmp/x")),
                    kind: CheckoutKind::Clone,
                    git: Err("git failed".into()),
                },
                Class::Protected,
            ),
        ];
        for (f, want) in cases {
            let (got, reason) = classify(&f, &mains, &roots);
            assert_eq!(got, want, "{:?} → {reason}", f.realpath);
            assert!(!reason.is_empty());
        }
    }
}
