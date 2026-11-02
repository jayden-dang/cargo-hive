//! Stored plans in `plans.json`. A plan is found by id and consumed at most once;
//! consuming it leaves a tombstone with the path list removed. The file is read and
//! written only under the state lock.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::platform::EntryKind;
use crate::state::{State, StateError, Tx};

const PLANS_FILE: &str = "plans.json";
pub const MAX_PLANS_BYTES: u64 = 4 * 1024 * 1024;
const KEEP_FOR: u64 = 30 * 86_400;
const MAX_TOMBSTONES: usize = 256;
const FNV_OFFSET: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x0100_0000_01b3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlanClass {
    Secondary,
    Protected,
    Orphan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BytesRec {
    pub freed: u64,
    pub still_shared: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedEntry {
    pub path: PathBuf,
    pub unit: String,
    pub kind: String,
    pub dev: u64,
    pub ino: u64,
}

pub fn kind_name(kind: EntryKind) -> &'static str {
    match kind {
        EntryKind::File => "file",
        EntryKind::Dir => "dir",
        EntryKind::Symlink => "symlink",
        EntryKind::Other => "other",
    }
}

pub fn kind_of(name: &str) -> Option<EntryKind> {
    match name {
        "file" => Some(EntryKind::File),
        "dir" => Some(EntryKind::Dir),
        "symlink" => Some(EntryKind::Symlink),
        "other" => Some(EntryKind::Other),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrphanScope {
    pub root_kind: String,
    pub root_dev: u64,
    pub root_ino: u64,
    pub dirs: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedTarget {
    pub path: PathBuf,
    pub owner: Option<PathBuf>,
    pub planned: Option<BytesRec>,
    pub entries: Vec<PlannedEntry>,
    pub orphan: Option<OrphanScope>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlanState {
    Live,
    Consumed { at: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub id: String,
    pub class: PlanClass,
    pub window_days: u64,
    pub made_at: u64,
    pub scope_repos: Vec<PathBuf>,
    pub planned: Option<BytesRec>,
    pub targets: Vec<PlannedTarget>,
    pub state: PlanState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup {
    Live(Plan),
    Consumed,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Taken {
    Plan(Plan),
    AlreadyConsumed,
    Unknown,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct PlansFile {
    plans: Vec<Plan>,
}

pub fn insert(
    state: &State,
    cancel: &dyn Fn() -> bool,
    plans: Vec<Plan>,
    now: u64,
) -> Result<(), StateError> {
    state.locked_or_cancelled(cancel, |tx| {
        let mut file = read(tx)?;
        prune(&mut file.plans, now);
        if let Some(id) = duplicate_id(&file.plans, &plans) {
            return Err(StateError::DuplicatePlan { id: id.to_string() });
        }
        file.plans.extend(plans);
        tx.write_bounded(PLANS_FILE, &file, MAX_PLANS_BYTES)
    })
}

pub fn get(state: &State, id: &str) -> Result<Lookup, StateError> {
    state.locked(|tx| {
        let file = read(tx)?;
        Ok(match file.plans.into_iter().find(|plan| plan.id == id) {
            Some(plan) => match plan.state {
                PlanState::Live => Lookup::Live(plan),
                PlanState::Consumed { .. } => Lookup::Consumed,
            },
            None => Lookup::Unknown,
        })
    })
}

pub fn consume(
    state: &State,
    cancel: &dyn Fn() -> bool,
    id: &str,
    now: u64,
) -> Result<Taken, StateError> {
    state.locked_or_cancelled(cancel, |tx| {
        let mut file = read(tx)?;
        let Some(plan) = file.plans.iter_mut().find(|plan| plan.id == id) else {
            return Ok(Taken::Unknown);
        };
        match plan.state {
            PlanState::Consumed { .. } => Ok(Taken::AlreadyConsumed),
            PlanState::Live => {
                let taken = plan.clone();
                plan.targets.clear();
                plan.state = PlanState::Consumed { at: now };
                tx.write(PLANS_FILE, &file)?;
                Ok(Taken::Plan(taken))
            }
        }
    })
}

pub fn new_id(now_nanos: u128, pid: u32, counter: u64) -> String {
    let mut hash = FNV_OFFSET;
    let bytes = now_nanos
        .to_le_bytes()
        .into_iter()
        .chain(pid.to_le_bytes())
        .chain(counter.to_le_bytes());
    for byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    format!("{hash:016x}")
}

fn read(tx: &Tx<'_>) -> Result<PlansFile, StateError> {
    tx.read_bounded(PLANS_FILE, MAX_PLANS_BYTES)
}

fn duplicate_id<'a>(existing: &'a [Plan], incoming: &'a [Plan]) -> Option<&'a str> {
    incoming.iter().enumerate().find_map(|(index, plan)| {
        let seen = existing.iter().any(|kept| kept.id == plan.id)
            || incoming[..index]
                .iter()
                .any(|earlier| earlier.id == plan.id);
        seen.then_some(plan.id.as_str())
    })
}

fn prune(plans: &mut Vec<Plan>, now: u64) {
    plans.retain(|plan| now.saturating_sub(plan.made_at) <= KEEP_FOR);
    let mut consumed: Vec<usize> = plans
        .iter()
        .enumerate()
        .filter(|(_, plan)| matches!(plan.state, PlanState::Consumed { .. }))
        .map(|(index, _)| index)
        .collect();
    if consumed.len() <= MAX_TOMBSTONES {
        return;
    }
    consumed.sort_by(|&left, &right| {
        plans[right]
            .made_at
            .cmp(&plans[left].made_at)
            .then(right.cmp(&left))
    });
    consumed.truncate(MAX_TOMBSTONES);
    consumed.sort_unstable();
    let mut next = 0;
    let mut index = 0;
    plans.retain(|plan| {
        let keep = if matches!(plan.state, PlanState::Consumed { .. }) {
            let kept = next < consumed.len() && consumed[next] == index;
            if kept {
                next += 1;
            }
            kept
        } else {
            true
        };
        index += 1;
        keep
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(label: &str) -> State {
        let d = std::path::PathBuf::from(format!(
            "/private/tmp/hive-it-{label}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        State::open_at(d).unwrap()
    }

    fn plan(id: &str, class: PlanClass, now: u64) -> Plan {
        Plan {
            id: id.into(),
            class,
            window_days: 7,
            made_at: now,
            scope_repos: vec![],
            planned: Some(BytesRec {
                freed: 10,
                still_shared: 0,
            }),
            targets: vec![PlannedTarget {
                path: "/t".into(),
                owner: Some("/o".into()),
                planned: None,
                entries: vec![PlannedEntry {
                    path: "/t/a".into(),
                    unit: "debug/x-1".into(),
                    kind: "file".into(),
                    dev: 1,
                    ino: 2,
                }],
                orphan: None,
            }],
            state: PlanState::Live,
        }
    }

    fn encoded(plans: &[Plan]) -> Vec<u8> {
        let mut bytes = serde_json::to_vec_pretty(&PlansFile {
            plans: plans.to_vec(),
        })
        .unwrap();
        bytes.push(b'\n');
        bytes
    }

    fn plan_with_pad(id: &str, pad: usize) -> Plan {
        let mut stored = plan(id, PlanClass::Secondary, 1_000);
        stored.targets[0].entries[0].unit = "u".repeat(pad);
        stored
    }

    #[test]
    fn insert_accepts_a_file_at_the_cap_and_refuses_one_byte_past() {
        let cap = MAX_PLANS_BYTES as usize;
        let bare = encoded(&[plan_with_pad("aaaaaaaaaaaaaaaa", 0)]);
        assert!(bare.len() < cap);
        let fitted = plan_with_pad("aaaaaaaaaaaaaaaa", cap - bare.len());
        assert_eq!(encoded(std::slice::from_ref(&fitted)).len(), cap);
        let at_cap = state("record-write-cap");
        insert(&at_cap, &|| false, vec![fitted], 1_000).unwrap();
        let path = at_cap.root().join(PLANS_FILE);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), MAX_PLANS_BYTES);
        assert!(matches!(
            get(&at_cap, "aaaaaaaaaaaaaaaa").unwrap(),
            Lookup::Live(_)
        ));

        let over = state("record-write-over");
        let kept = plan("bbbbbbbbbbbbbbbb", PlanClass::Secondary, 1_000);
        insert(&over, &|| false, vec![kept.clone()], 1_000).unwrap();
        let path = over.root().join(PLANS_FILE);
        let before = std::fs::read(&path).unwrap();
        let with_extra = encoded(&[kept, plan_with_pad("aaaaaaaaaaaaaaaa", 0)]);
        assert!(with_extra.len() < cap);
        let extra = plan_with_pad("aaaaaaaaaaaaaaaa", cap + 1 - with_extra.len());
        assert_eq!(
            encoded(&[
                plan("bbbbbbbbbbbbbbbb", PlanClass::Secondary, 1_000),
                extra.clone()
            ])
            .len(),
            cap + 1
        );
        let err = insert(&over, &|| false, vec![extra], 1_000).unwrap_err();
        let text = err.to_string();
        assert!(text.contains("plans.json"), "{text}");
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(matches!(
            get(&over, "bbbbbbbbbbbbbbbb").unwrap(),
            Lookup::Live(_)
        ));
        assert!(matches!(
            get(&over, "aaaaaaaaaaaaaaaa").unwrap(),
            Lookup::Unknown
        ));
    }

    #[test]
    fn plans_json_at_the_cap_loads_and_one_byte_past_names_the_file() {
        let s = state("record-cap");
        assert!(matches!(
            get(&s, "aaaaaaaaaaaaaaaa").unwrap(),
            Lookup::Unknown
        ));
        let path = s.root().join(PLANS_FILE);
        let cap = MAX_PLANS_BYTES as usize;
        let mut body = b"{\"plans\":[]".to_vec();
        body.extend(std::iter::repeat_n(b' ', cap - body.len() - 1));
        body.push(b'}');
        assert_eq!(body.len(), cap);
        std::fs::write(&path, &body).unwrap();
        assert!(matches!(
            get(&s, "aaaaaaaaaaaaaaaa").unwrap(),
            Lookup::Unknown
        ));
        body.push(b'x');
        std::fs::write(&path, &body).unwrap();
        let err = get(&s, "aaaaaaaaaaaaaaaa").unwrap_err();
        let text = err.to_string();
        assert!(text.contains("plans.json"), "{text}");
        assert!(text.contains("larger than"), "{text}");
    }

    #[test]
    fn a_stored_plan_is_found_and_consumed_exactly_once() {
        let s = state("record-once");
        insert(
            &s,
            &|| false,
            vec![plan("aaaaaaaaaaaaaaaa", PlanClass::Secondary, 100)],
            100,
        )
        .unwrap();
        assert!(matches!(
            get(&s, "aaaaaaaaaaaaaaaa").unwrap(),
            Lookup::Live(_)
        ));
        let first = consume(&s, &|| false, "aaaaaaaaaaaaaaaa", 101).unwrap();
        assert!(matches!(first, Taken::Plan(p) if p.targets.len() == 1));
        assert!(matches!(
            consume(&s, &|| false, "aaaaaaaaaaaaaaaa", 102).unwrap(),
            Taken::AlreadyConsumed
        ));
        assert!(matches!(
            get(&s, "aaaaaaaaaaaaaaaa").unwrap(),
            Lookup::Consumed
        ));
        assert!(matches!(
            get(&s, "bbbbbbbbbbbbbbbb").unwrap(),
            Lookup::Unknown
        ));
    }

    #[test]
    fn a_consumed_plan_keeps_a_tombstone_without_its_paths() {
        let s = state("record-tombstone");
        insert(
            &s,
            &|| false,
            vec![plan("aaaaaaaaaaaaaaaa", PlanClass::Protected, 100)],
            100,
        )
        .unwrap();
        consume(&s, &|| false, "aaaaaaaaaaaaaaaa", 101).unwrap();
        let text = std::fs::read_to_string(s.root().join("plans.json")).unwrap();
        assert!(
            text.contains("aaaaaaaaaaaaaaaa") && !text.contains("/t/a"),
            "{text}"
        );
    }

    #[test]
    fn prune_drops_old_entries_and_extra_tombstones_but_never_a_young_live_plan() {
        let s = state("record-prune");
        let day = 86_400;
        let now = 100 * day;
        let exactly = now - 30 * day;
        let just_over = exactly - 1;
        insert(
            &s,
            &|| false,
            vec![
                plan("aaaaaaaaaaaaaaaa", PlanClass::Secondary, exactly),
                plan("bbbbbbbbbbbbbbbb", PlanClass::Secondary, just_over),
                plan("cccccccccccccccc", PlanClass::Secondary, now),
            ],
            now,
        )
        .unwrap();
        // prune runs before the new plans are stored, so a later insert is what drops them
        insert(
            &s,
            &|| false,
            vec![plan("dddddddddddddddd", PlanClass::Secondary, now)],
            now,
        )
        .unwrap();
        assert!(
            matches!(get(&s, "aaaaaaaaaaaaaaaa").unwrap(), Lookup::Live(_)),
            "a plan aged exactly 30 days is kept"
        );
        assert!(
            matches!(get(&s, "bbbbbbbbbbbbbbbb").unwrap(), Lookup::Unknown),
            "a plan aged 30 days plus one second is dropped"
        );
        assert!(matches!(
            get(&s, "cccccccccccccccc").unwrap(),
            Lookup::Live(_)
        ));
        assert!(matches!(
            get(&s, "dddddddddddddddd").unwrap(),
            Lookup::Live(_)
        ));

        let tombs: Vec<_> = (1..=256u64)
            .map(|n| plan(&format!("{n:016x}"), PlanClass::Secondary, now))
            .collect();
        insert(&s, &|| false, tombs, now).unwrap();
        for n in 1..=256u64 {
            consume(&s, &|| false, &format!("{n:016x}"), now).unwrap();
        }
        insert(
            &s,
            &|| false,
            vec![plan("eeeeeeeeeeeeeeee", PlanClass::Secondary, now)],
            now,
        )
        .unwrap();
        for n in 1..=256u64 {
            assert!(
                matches!(get(&s, &format!("{n:016x}")).unwrap(), Lookup::Consumed),
                "tombstone {n} was dropped inside the 256 cap"
            );
        }

        insert(
            &s,
            &|| false,
            vec![plan("ffffffffffffffff", PlanClass::Secondary, now - 10)],
            now,
        )
        .unwrap();
        consume(&s, &|| false, "ffffffffffffffff", now).unwrap();
        insert(
            &s,
            &|| false,
            vec![plan("1111111111111111", PlanClass::Secondary, now)],
            now,
        )
        .unwrap();
        assert!(
            matches!(get(&s, "ffffffffffffffff").unwrap(), Lookup::Unknown),
            "the 257th tombstone is the one dropped"
        );
        let kept = (1..=256u64)
            .filter(|n| matches!(get(&s, &format!("{n:016x}")).unwrap(), Lookup::Consumed))
            .count();
        assert_eq!(kept, 256);
        assert!(matches!(
            get(&s, "cccccccccccccccc").unwrap(),
            Lookup::Live(_)
        ));
        assert!(matches!(
            get(&s, "1111111111111111").unwrap(),
            Lookup::Live(_)
        ));
    }

    #[test]
    fn a_cancelled_write_leaves_the_file_as_it_was() {
        let s = state("record-cancelled");
        insert(
            &s,
            &|| false,
            vec![plan("aaaaaaaaaaaaaaaa", PlanClass::Secondary, 100)],
            100,
        )
        .unwrap();
        let before = std::fs::read_to_string(s.root().join("plans.json")).unwrap();
        // another holder of the state lock, then a cancelled insert: the lock is contended, so cancel is honoured
        let r = s.locked(|_| {
            Ok(insert(
                &s,
                &|| true,
                vec![plan("bbbbbbbbbbbbbbbb", PlanClass::Secondary, 100)],
                100,
            ))
        });
        assert!(matches!(r.unwrap(), Err(StateError::Cancelled)));
        assert_eq!(
            std::fs::read_to_string(s.root().join("plans.json")).unwrap(),
            before
        );
    }

    #[test]
    fn a_duplicate_id_is_refused() {
        let s = state("record-duplicate");
        insert(
            &s,
            &|| false,
            vec![plan("aaaaaaaaaaaaaaaa", PlanClass::Secondary, 100)],
            100,
        )
        .unwrap();
        let before = std::fs::read_to_string(s.root().join("plans.json")).unwrap();
        let r = insert(
            &s,
            &|| false,
            vec![plan("aaaaaaaaaaaaaaaa", PlanClass::Secondary, 100)],
            100,
        );
        let err = r.unwrap_err();
        assert!(matches!(
            err,
            StateError::DuplicatePlan { ref id } if id == "aaaaaaaaaaaaaaaa"
        ));
        let text = err.to_string();
        assert!(text.contains("aaaaaaaaaaaaaaaa"), "{text}");
        assert!(text.contains("collided"), "{text}");
        assert!(text.contains("cargo hive gc --plan"), "{text}");
        assert!(!text.contains("counter"), "{text}");
        assert!(
            !matches!(err, StateError::Malformed { .. }),
            "a collision is a retry refusal, not a malformed file: {text}"
        );
        assert_eq!(
            std::fs::read_to_string(s.root().join("plans.json")).unwrap(),
            before
        );
    }

    #[test]
    fn ids_are_sixteen_lowercase_hex_characters_and_entry_kinds_round_trip() {
        let id = new_id(1_700_000_000_123_456_789, 4242, 7);
        assert_eq!(id.len(), 16);
        assert!(id
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)));
        assert_ne!(id, new_id(1_700_000_000_123_456_789, 4242, 8));
        for k in ["file", "dir", "symlink", "other"] {
            assert_eq!(kind_name(kind_of(k).unwrap()), k);
        }
    }
}
