//! Receipts hold counted or measured values only. Bench is the one command whose receipt
//! holds integer milliseconds; there is no float or duration slot.
//! Plan and apply receipts are version 3. Every other command writes version 2.

use crate::platform::clock::utc_timestamp;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::SystemTime;

/// Schema version written by every command except plan and apply, which are version 3.
pub const SCHEMA: u32 = 2;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(untagged)]
pub enum Measured {
    Int(u64),
    Bool(bool),
    Text(String),
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct Scope {
    pub repos: Vec<PathBuf>,
    pub checkouts: Vec<PathBuf>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct CheckRecord {
    pub id: String,
    pub verdict: String,
    pub measured: BTreeMap<String, Measured>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub kind: String,
    pub path: Option<PathBuf>,
    pub detail: String,
    #[serde(default)]
    pub measured: BTreeMap<String, Measured>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Unmeasured {
    pub field: String,
    pub reason: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    Applied,
    Partial,
    Refused,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct PlanRecord {
    pub id: String,
    pub class: String,
    pub outcome: Option<Outcome>,
    pub reason: Option<String>,
    pub targets: Vec<PathBuf>,
    pub paths: u64,
    pub counts: BTreeMap<String, u64>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ReasonRecord {
    pub what: String,
    pub clears_when: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct UndecidedRecord {
    pub target: PathBuf,
    pub reasons: Vec<ReasonRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanTierBody {
    pub plans: Vec<PlanRecord>,
    pub undecided: Vec<UndecidedRecord>,
    pub nothing_to_do: Vec<PathBuf>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    pub schema: u32,
    pub hive_version: String,
    pub macos_version: Option<String>,
    pub toolchain: Option<String>,
    pub command: String,
    pub ts: String,
    pub scope: Scope,
    pub counts: BTreeMap<String, u64>,
    pub checks: Vec<CheckRecord>,
    pub findings: Vec<Finding>,
    pub unmeasured: Vec<Unmeasured>,
    pub notes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plans: Option<Vec<PlanRecord>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undecided: Option<Vec<UndecidedRecord>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nothing_to_do: Option<Vec<PathBuf>>,
}

pub struct ReceiptBuilder {
    r: Receipt,
}

impl Receipt {
    #[allow(clippy::new_ret_no_self)]
    pub fn new(command: &str, scope: Scope) -> ReceiptBuilder {
        Receipt::with_os(command, scope, crate::platform::os_version())
    }

    pub fn with_os(command: &str, scope: Scope, os: Option<String>) -> ReceiptBuilder {
        ReceiptBuilder {
            r: Receipt {
                schema: SCHEMA,
                hive_version: env!("CARGO_PKG_VERSION").to_string(),
                macos_version: os,
                toolchain: None,
                command: command.to_string(),
                ts: utc_timestamp(SystemTime::now()),
                scope,
                counts: BTreeMap::new(),
                checks: Vec::new(),
                findings: Vec::new(),
                unmeasured: Vec::new(),
                notes: Vec::new(),
                plans: None,
                undecided: None,
                nothing_to_do: None,
            },
        }
    }
}

impl ReceiptBuilder {
    pub fn count(&mut self, name: &str, n: u64) -> &mut Self {
        self.r.counts.insert(name.to_string(), n);
        self
    }
    pub fn check(&mut self, c: CheckRecord) -> &mut Self {
        self.r.checks.push(c);
        self
    }
    pub fn finding(
        &mut self,
        kind: &str,
        path: Option<PathBuf>,
        detail: impl Into<String>,
    ) -> &mut Self {
        self.r.findings.push(Finding {
            kind: kind.to_string(),
            path,
            detail: detail.into(),
            measured: BTreeMap::new(),
        });
        self
    }
    pub fn finding_measured(
        &mut self,
        kind: &str,
        path: Option<PathBuf>,
        detail: impl Into<String>,
        measured: BTreeMap<String, Measured>,
    ) -> &mut Self {
        self.r.findings.push(Finding {
            kind: kind.to_string(),
            path,
            detail: detail.into(),
            measured,
        });
        self
    }
    pub fn unmeasured(&mut self, field: &str, reason: impl Into<String>) -> &mut Self {
        self.r.unmeasured.push(Unmeasured {
            field: field.to_string(),
            reason: reason.into(),
        });
        self
    }
    pub fn note(&mut self, text: impl Into<String>) -> &mut Self {
        self.r.notes.push(text.into());
        self
    }
    pub fn toolchain(&mut self, release: Option<String>) -> &mut Self {
        self.r.toolchain = release;
        self
    }
    pub fn plan_tier(&mut self, body: PlanTierBody) -> &mut Self {
        self.r.schema = 3;
        self.r.plans = Some(body.plans);
        self.r.undecided = Some(body.undecided);
        self.r.nothing_to_do = Some(body.nothing_to_do);
        self
    }
    pub fn scope_mut(&mut self) -> &mut Scope {
        &mut self.r.scope
    }
    pub fn finish(mut self) -> Receipt {
        if self.r.macos_version.is_none() {
            self.r.unmeasured.push(Unmeasured {
                field: "macos_version".into(),
                reason: "sw_vers -productVersion could not be run".into(),
            });
        }
        self.r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has_float(v: &serde_json::Value) -> bool {
        match v {
            serde_json::Value::Number(n) => n.is_f64(),
            serde_json::Value::Array(a) => a.iter().any(has_float),
            serde_json::Value::Object(o) => o.values().any(has_float),
            _ => false,
        }
    }

    #[test]
    fn a_receipt_carries_only_counted_values() {
        let mut b = Receipt::with_os("status", Scope::default(), Some("27.0.1".into()));
        b.count("targets", 3).count("units", u64::MAX);
        b.check(CheckRecord {
            id: "a".into(),
            verdict: "pass".into(),
            measured: [("private_bytes".to_string(), Measured::Int(0))].into(),
        });
        b.unmeasured("sizes", "--no-sizes");
        let value = serde_json::to_value(b.finish()).unwrap();
        assert!(!has_float(&value));
        assert_eq!(value["counts"]["units"], serde_json::json!(u64::MAX));
    }

    #[test]
    fn a_finding_carries_measured_values_and_the_schema_is_2() {
        let mut measured = BTreeMap::new();
        measured.insert("copies".into(), Measured::Int(3));
        measured.insert("crate".into(), Measured::Text("persistence".into()));
        let mut b = Receipt::with_os("explain", Scope::default(), Some("27.0.1".into()));
        b.finding_measured(
            "copies.group",
            Some(PathBuf::from("/profile")),
            "3 copies",
            measured,
        );
        let value = serde_json::to_value(b.finish()).unwrap();
        assert_eq!(value["schema"], 2);
        assert_eq!(value["findings"][0]["kind"], "copies.group");
        assert_eq!(value["findings"][0]["measured"]["copies"], 3);
        assert_eq!(value["findings"][0]["measured"]["crate"], "persistence");
        assert_eq!(value["findings"][0]["detail"], "3 copies");
    }

    #[test]
    fn a_finding_without_measured_values_serializes_an_empty_map() {
        let mut b = Receipt::with_os("status", Scope::default(), Some("27.0.1".into()));
        b.finding("target.owned", Some(PathBuf::from("/t")), "owned by /c");
        let value = serde_json::to_value(b.finish()).unwrap();
        assert_eq!(value["schema"], 2);
        assert_eq!(value["findings"][0]["measured"], serde_json::json!({}));
    }

    #[test]
    fn a_schema_1_finding_without_measured_still_parses() {
        let raw = r#"{"kind":"target.owned","path":"/t","detail":"owned"}"#;
        let finding: Finding = serde_json::from_str(raw).unwrap();
        assert!(finding.measured.is_empty());
        assert_eq!(finding.kind, "target.owned");
    }

    #[test]
    fn no_receipt_field_can_express_time_saved() {
        let value =
            serde_json::to_value(Receipt::with_os("x", Scope::default(), None).finish()).unwrap();
        for key in value.as_object().unwrap().keys() {
            for banned in ["saved", "duration", "seconds", "elapsed", "estimate"] {
                assert!(!key.contains(banned), "field {key}");
            }
        }
    }

    #[test]
    fn an_unavailable_os_version_is_reported_as_unmeasured_with_a_reason() {
        let r = Receipt::with_os("x", Scope::default(), None).finish();
        assert_eq!(r.macos_version, None);
        assert!(r
            .unmeasured
            .iter()
            .any(|u| u.field == "macos_version" && !u.reason.is_empty()));
    }

    #[test]
    fn a_plan_tier_receipt_is_schema_three_and_carries_its_groups() {
        let mut b = Receipt::with_os("gc --plan", Scope::default(), Some("27.0.1".into()));
        b.plan_tier(PlanTierBody {
            plans: vec![],
            undecided: vec![],
            nothing_to_do: vec![],
        });
        let v = serde_json::to_value(b.finish()).unwrap();
        assert_eq!(v["schema"], 3);
        for key in ["plans", "undecided", "nothing_to_do"] {
            assert!(v[key].as_array().unwrap().is_empty(), "{key}");
        }
    }

    #[test]
    fn other_receipts_keep_schema_two_and_omit_the_plan_keys() {
        let v = serde_json::to_value(Receipt::with_os("status", Scope::default(), None).finish())
            .unwrap();
        assert_eq!(v["schema"], 2);
        for key in ["plans", "undecided", "nothing_to_do"] {
            assert!(v.get(key).is_none(), "{key}");
        }
    }

    #[test]
    fn outcomes_serialize_by_their_names_and_old_log_lines_still_parse() {
        assert_eq!(
            serde_json::to_string(&Outcome::Refused).unwrap(),
            "\"refused\""
        );
        let old =
            serde_json::to_string(&Receipt::with_os("status", Scope::default(), None).finish())
                .unwrap();
        let parsed: Receipt = serde_json::from_str(&old).unwrap();
        assert!(parsed.plans.is_none());
    }

    #[test]
    fn a_populated_plan_tier_receipt_names_no_time_or_estimate() {
        let mut counts = std::collections::BTreeMap::new();
        counts.insert("paths_deleted".to_string(), 3u64);
        counts.insert("freed_bytes".to_string(), 4096u64);
        let mut b = Receipt::with_os("apply", Scope::default(), None);
        b.plan_tier(PlanTierBody {
            plans: vec![PlanRecord {
                id: "0123456789abcdef".into(),
                class: "secondary".into(),
                outcome: Some(Outcome::Applied),
                reason: None,
                targets: vec!["/t".into()],
                paths: 3,
                counts,
            }],
            undecided: vec![UndecidedRecord {
                target: "/u".into(),
                reasons: vec![ReasonRecord {
                    what: "w".into(),
                    clears_when: "c".into(),
                }],
            }],
            nothing_to_do: vec!["/n".into()],
        });
        let text = serde_json::to_string(&b.finish()).unwrap();
        for word in ["saved", "duration", "seconds", "elapsed", "estimate"] {
            assert!(!text.contains(word), "{word}");
        }
    }
}
