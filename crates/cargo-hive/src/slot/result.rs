//! The slot projection carries public results; its audit uses the existing receipt format.

use cargo_hive_core::receipt::{Receipt, Scope};
use serde::Serialize;
use std::io::{self, Write};
use std::path::PathBuf;

pub(super) const FRESH_FALLBACK: &str = "Create a separate fresh secondary checkout and use the installed guide's verified seed fallback.";

#[derive(Serialize)]
pub(super) struct SlotResult {
    schema: u32,
    #[serde(skip)]
    pub command: &'static str,
    pub kind: &'static str,
    pub repo: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pool_id: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub slots: Vec<SlotStatus>,
    #[serde(flatten)]
    pub transition: Option<SlotTransition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempt: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    #[serde(rename = "commit", skip_serializing_if = "Option::is_none")]
    pub fallback_commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registry_units: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub use_policy: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_action: Option<&'static str>,
    #[serde(skip)]
    pub code: i32,
}

#[derive(Serialize)]
pub(super) struct SlotTransition {
    pub slot: u8,
    pub generation: u64,
    pub phase: &'static str,
    pub commit: String,
    pub current: bool,
}

#[derive(Serialize)]
pub(super) struct SlotStatus {
    pub number: u8,
    pub path: PathBuf,
    pub phase: &'static str,
    pub generation: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_action: Option<&'static str>,
}

impl SlotResult {
    pub fn new(
        command: &'static str,
        kind: &'static str,
        repo: Option<PathBuf>,
        code: i32,
    ) -> Self {
        Self {
            schema: 1,
            command,
            kind,
            repo: repo.filter(|path| path.to_str().is_some()),
            pool_id: None,
            slots: Vec::new(),
            transition: None,
            attempt: None,
            current: None,
            path: None,
            fallback_commit: None,
            registry_units: None,
            use_policy: None,
            detail: None,
            next_action: None,
            code,
        }
    }

    pub fn audit_receipt(&self) -> Receipt {
        let scope = Scope {
            repos: self.repo.clone().into_iter().collect(),
            checkouts: self
                .path
                .iter()
                .filter(|_| self.registry_units.is_some())
                .cloned()
                .collect(),
        };
        let mut receipt = Receipt::new(self.command, scope);
        receipt.count(
            "slots",
            self.slots.len() as u64 + u64::from(self.transition.is_some()),
        );
        if let Some(registry_units) = self.registry_units {
            receipt.count("registry_units", registry_units);
        }
        if self.code != 0 {
            receipt.finding("error", None, self.detail.as_deref().unwrap_or(self.kind));
        }
        if let Some(action) = self.next_action {
            receipt.note(action);
        }
        receipt.finish()
    }

    pub fn emit(&self, json: bool) -> i32 {
        if json {
            let line = match serde_json::to_string(self) {
                Ok(line) => line,
                Err(_) => {
                    eprintln!("cargo-hive: could not encode the slot result");
                    return 1;
                }
            };
            let mut stdout = io::stdout().lock();
            if writeln!(stdout, "{line}")
                .and_then(|()| stdout.flush())
                .is_err()
            {
                let _ = writeln!(
                    io::stderr().lock(),
                    "cargo-hive: could not deliver the slot result"
                );
                return 1;
            }
            return self.code;
        }

        match &self.detail {
            Some(detail) if self.code != 0 => eprintln!("cargo-hive: {detail}"),
            Some(detail) => println!("{detail}"),
            None => println!("{}", self.kind),
        }
        if let Some(transition) = &self.transition {
            let status = if transition.current {
                "current"
            } else {
                "historical"
            };
            println!(
                "  slot {} generation {} {} at {} ({status})",
                transition.slot, transition.generation, transition.phase, transition.commit
            );
        }
        for slot in &self.slots {
            println!(
                "  slot {} generation {} {} {}",
                slot.number,
                slot.generation,
                slot.phase,
                slot.path.display()
            );
            if let Some(action) = slot.next_action {
                println!("    {action}");
            }
        }
        if let Some(action) = self.next_action {
            println!("{action}");
        }
        self.code
    }
}
