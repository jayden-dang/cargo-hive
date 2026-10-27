//! Receipt schema v1: counted or measured values only. There is no float or duration slot.

use crate::state::utc_timestamp;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::SystemTime;

pub const SCHEMA: u32 = 1;

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
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Unmeasured {
    pub field: String,
    pub reason: String,
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
}
