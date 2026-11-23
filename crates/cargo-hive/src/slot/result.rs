//! The slot projection carries public results; its audit uses the existing receipt format.

use cargo_hive_core::receipt::{Receipt, Scope};
use serde::Serialize;
use std::path::PathBuf;

pub(super) const FRESH_FALLBACK: &str =
    "Create a separate fresh secondary checkout and run `cargo hive seed <checkout>` there.";

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_action: Option<&'static str>,
    #[serde(skip)]
    pub code: i32,
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
            detail: None,
            next_action: None,
            code,
        }
    }

    pub fn audit_receipt(&self) -> Receipt {
        let scope = Scope {
            repos: self.repo.clone().into_iter().collect(),
            checkouts: Vec::new(),
        };
        let mut receipt = Receipt::new(self.command, scope);
        receipt.count("slots", self.slots.len() as u64);
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
            match serde_json::to_string(self) {
                Ok(line) => println!("{line}"),
                Err(_) => {
                    eprintln!("cargo-hive: could not encode the slot result");
                    return 1;
                }
            }
            return self.code;
        }

        match &self.detail {
            Some(detail) if self.code != 0 => eprintln!("cargo-hive: {detail}"),
            Some(detail) => println!("{detail}"),
            None => println!("{}", self.kind),
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
