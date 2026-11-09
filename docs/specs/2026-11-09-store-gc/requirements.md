# Requirements: Store GC (bare `cargo hive gc` keeps hive's store bounded)

Feature code: STOREGC
Status: Draft
Date: 2026-11-09

<!--
Rules:
- Feature code: 2-12 chars, A-Z0-9, starts with a letter, unique repo-wide.
  Register it in docs/specs/INDEX.md before use.
- Every acceptance criterion gets a hierarchical ID: <CODE>-<story>.<criterion>.
- Criteria use EARS phrasing:
    WHEN <event/condition> THE SYSTEM SHALL <behavior>          (event-driven)
    WHILE <state> THE SYSTEM SHALL <behavior>                   (state-driven)
    IF <unwanted condition> THEN THE SYSTEM SHALL <behavior>    (unwanted behavior)
    WHERE <feature is included> THE SYSTEM SHALL <behavior>     (optional feature)
    THE SYSTEM SHALL <behavior>                                 (ubiquitous)
- Guard requirements protect existing behavior this feature touches:
    WHEN <condition> THE SYSTEM SHALL CONTINUE TO <existing behavior>
- IDs are immutable once Status is Approved. Retire a requirement by striking it
  through (~~**CODE-N.M**~~ reason) — never renumber.
-->

Roadmap item: ROAD-10 (MILE-3, `docs/roadmap/INDEX.md`), tracker issue #5.

Sources:
- The close package of the ROAD-10 frame, `.skills/_pending-store-gc/auto/close-package.md`, approved by store-gc-interpret on 2026-11-09 under the unattended charter. It is not owner-approved. The ledger beside it (`ledger.md`) holds every Lock, Weigh, agent-decided and still-open line.
- The owner's Store GC decision of 2026-10-25 (`.skills/CORE/close-package.md:32`): rows record size, `filled_at` and `last_live_at`; `last_live_at` comes from closure scans; evict after 30 days not live anywhere; drop a rustc generation that no checkout uses; the GB cap counts bytes eviction would actually free (D7); never evict live units; if the live set exceeds the cap, report and stop. Defaults: store budget 40 GB, store max age 30 days (`docs/product/vision.md:71`; `.skills/CORE/close-package.md:75`).
- Locks this feature builds on: GOAL-4, GOAL-6, GOAL-7; ARCH-2, 4, 8, 10, 11, 14, 15, 16, 17, 18, 19; D7 of `.skills/CORE/close-package.md`; the liveness verdicts of LIVE; the store, entry and toolchain table of SEED; the auto tier of AUTOGC.

Labels. A value store-gc-interpret picked is marked *(Weigh Wn/An)*. A value store-gc-work chose under the charter's fork step 4, the option that leaves every documented contract as it is, is marked *(agent-decided)*. Neither is the owner's choice.

Terms (defined in `CONTEXT.md`): store, entry, store key, toolchain value, toolchain table, fill, seed, closure liveness, Undecided, auto tier, receipt. This file uses three more:
- **Store eviction:** the part of a bare `cargo hive gc` run that removes entries from the store.
- **Held key:** a store key that store eviction may not evict in this run, by STOREGC-1.2, STOREGC-4.2 or STOREGC-4.3.
- **Freeable bytes:** for an entry, the bytes its deletion would return to the volume, counted as gc counts freed bytes (D7, ARCH-11).

## 1. Evict entries nothing has used for 30 days

**Story:** As a developer or agent, I want bare `cargo hive gc` to remove store entries that no target has held for 30 days, so that the store sheds what nothing uses without my deleting it by hand.

- **STOREGC-1.1** WHEN `cargo hive gc` runs with no repository argument and without `--plan` THE SYSTEM SHALL run store eviction as stories 1 to 4 require, in the same command as its target deletions.
- **STOREGC-1.2** WHEN store eviction runs THE SYSTEM SHALL compute closure liveness, with the GC window of `gc_window_days` (default 7 days), over every target that AUTOGC-4.1 names for the same run, orphan candidates, unresolved and cache-root targets included, and also those `gc` leaves untouched for their volume (AUTOGC-4.5) or for an unsupported profile dir (AUTOGC-4.3). It SHALL treat as held the store key (toolchain value, name-meta, fingerprint hash) of every live unit, read from that unit's fingerprint. *(Owner 2026-10-25 + L1, Weigh W8: every target bare gc walks, `.skills/CORE/close-package.md:32`; agent-decided: the extension to targets gc skips for their volume or profile)*
- **STOREGC-1.3** WHEN store eviction finds that a held key of STOREGC-1.2 has a row in `store.db` THE SYSTEM SHALL set that row's `last_live_at` to the time of this run, written through the `state` module under its lock, while `liveness()` itself writes nothing.
- **STOREGC-1.4** WHEN store eviction runs THE SYSTEM SHALL evict every entry whose key is not held and whose `last_live_at`, after STOREGC-1.3, is more than 30 days before the time of this run. *(agent-decided: 30 days is a constant in this run; a config key is a persisted-format change, the charter's hard stop.)*
- **STOREGC-1.5** WHEN store eviction evicts an entry THE SYSTEM SHALL remove that entry's directory under `<state root>/store/<toolchain value>/` and its row in `store.db`.
- **STOREGC-1.6** WHEN store eviction meets an entry directory that has no row in `store.db`, whose key is not held and that no fill is publishing (STOREGC-6.1) THE SYSTEM SHALL evict it as STOREGC-1.5 does. A rowless directory whose key is held is left for fill to adopt (SEED design decision 6). *(agent-decided: a held key keeps adoption's meaning; an unheld one would otherwise sit outside every bound)*
- **STOREGC-1.7** IF `gc_window_days` in `config.json` is outside 1 to 365 days THEN THE SYSTEM SHALL evict nothing from the store in this run, still make the run's target deletions, and report one finding with the reason `seed` gives for the same value. *(agent-decided)*
- **STOREGC-1.8** WHEN store eviction runs THE SYSTEM SHALL leave everything under `<state root>/store/.tmp/` to fill's one-day sweep (SEED design decision 5), and report the allocated bytes under it as a count. *(agent-decided)*

## 2. Drop a toolchain generation no checkout uses

**Story:** As a developer or agent, I want the first bare `cargo hive gc` after every checkout has moved to a new toolchain to remove the old toolchain's store entries, so that an upgrade does not leave a generation of entries no seed can clone.

- **STOREGC-2.1** WHEN store eviction runs THE SYSTEM SHALL count a toolchain value as in use when the toolchain table maps to it the whole `rustc -vV` text of either of these: (a) the toolchain each checkout of every registered repo that is not missing selects in its own directory, with any `RUSTUP_TOOLCHAIN` of hive's caller removed; (b) the toolchain `seed` would resolve, the caller's `RUSTUP_TOOLCHAIN` kept as `drop_directory_toolchain` keeps it today. Lookup (a) is the one `rustup` and `rustc` lookup that removes `RUSTUP_TOOLCHAIN` and `RUSTUP_TOOLCHAIN_SOURCE` whatever their source, a stated exception to CORE-1.12 for this lookup only; every other child keeps CORE-1.11 and CORE-1.12. *(Weigh W9, amending W3: the rustup proxy hands every `cargo` subcommand `RUSTUP_TOOLCHAIN`, which beats a checkout's own directory, `src/doctor.rs:354-366`; verified on rustup 1.29.3, ledger V1)*
- **STOREGC-2.2** WHEN a toolchain value that entries in the store carry is not in use THE SYSTEM SHALL evict every entry of that value whose key is not held, whatever its `last_live_at`. *(Weigh W3)*
- **STOREGC-2.3** WHEN no row and no entry directory of a toolchain value that is not in use remains after eviction THE SYSTEM SHALL remove every toolchain table row that maps to that value, and SHALL keep the rows of a value while any row or entry directory of it remains. *(Weigh W7)*
- **STOREGC-2.4** IF lookup (a) or lookup (b) of STOREGC-2.1 fails for any such checkout, for a reason other than a cancel, or the run meets a fault `apply` would refuse on (a checkout-walk fault that `split_checkout_faults` keeps, or a target-scan fault that `faults_that_can_hide_an_owner` keeps) THEN THE SYSTEM SHALL drop no toolchain value in this run, still apply stories 1 and 3, and report one finding per cause naming the checkout or path and the error. *(Weigh W3, W9)*
- **STOREGC-2.5** IF no checkout's toolchain was resolved in this run, because no repo is registered or every checkout is missing THEN THE SYSTEM SHALL drop no toolchain value in this run, and still apply stories 1 and 3. *(agent-decided)*

## 3. Keep the store under its cap

**Story:** As a developer or agent, I want bare `cargo hive gc` to bring the store down to 40 GB of freeable bytes when it is over, so that the store has a ceiling I can rely on.

The cap is a constant of the CLI. The eviction step takes it as a parameter, and tests and the demonstration of this story run that step with a small cap; no environment variable, flag or key exposes it. *(Weigh W10; agent-decided: a test seam inside the crate, not a setting)*

- **STOREGC-3.1** WHEN store eviction has applied stories 1 and 2 THE SYSTEM SHALL measure the freeable bytes of every remaining entry, counting each inode once, with bytes a surviving clone or hard link still holds counted apart and not as freeable.
- **STOREGC-3.2** WHILE the freeable bytes of the remaining entries total more than 40,000,000,000 bytes THE SYSTEM SHALL evict whole entries whose keys are not held, oldest `last_live_at` first, ties broken by one total order on the store key, and SHALL stop at the first point where the total is at or under that cap, except in a run STOREGC-6.1 skips. *(agent-decided: 40 GB read as 40 × 10⁹ bytes, a constant in this run, and the oldest-first order; Weigh A2: the tie-break)*
- **STOREGC-3.3** IF the remaining entries are all held and their freeable bytes still total more than the cap THEN THE SYSTEM SHALL evict none of them, and report one finding giving that total and the cap.
- **STOREGC-3.4** IF doctor check (c), the eviction-accounting capability, has not passed for the active toolchain THEN THE SYSTEM SHALL evict nothing for the cap, still apply stories 1 and 2, and record the cap step in `unmeasured` with the reason "doctor check (c) did not pass". *(agent-decided)*

## 4. Never evict what a target may still use

**Story:** As a developer or agent, I want bare `cargo hive gc` to keep every store entry a target may still be using, even a target whose liveness hive cannot decide, so that eviction never takes an entry out from under a working lane.

- **STOREGC-4.1** THE SYSTEM SHALL evict no entry whose key is held, by any rule of stories 1 to 3.
- **STOREGC-4.2** WHEN a target STOREGC-1.2 reads is Undecided THE SYSTEM SHALL treat as held the store key of every unit its supported profile dirs hold, read from each unit's fingerprint, and report one finding naming the target and what clears its Undecided reason. *(Weigh W1)*
- **STOREGC-4.3** WHEN a unit's store key cannot be formed, because it is a not-current unit with an empty or absent hash file, or because its profile dir is unsupported (any file in it unreadable or outside the format signature) THE SYSTEM SHALL treat as held the key of every entry whose name-meta is that unit's directory name under `.fingerprint/`, and for an unsupported profile dir, every directory name under its `.fingerprint/`. A profile dir with no `.fingerprint/` holds nothing, since no entry can come from it (SEED lock 7). *(Weigh W1, extended by store-gc-work to the profile level the code fails at: `src/fingerprint.rs:280-286,504-512`)*
- **STOREGC-4.4** IF a target STOREGC-1.2 reads is unreadable, the listing of a `.fingerprint/`, `deps/` or `examples/` directory in it fails, or the run meets a fault `apply` would refuse on (STOREGC-2.4) THEN THE SYSTEM SHALL evict no entry in this run, and report one finding per cause naming the path and what clears it. *(Weigh W1)*
- **STOREGC-4.5** WHEN store eviction holds the whole store (STOREGC-4.4), blocks a toolchain drop (STOREGC-2.4) or stops above the cap (STOREGC-3.3) THE SYSTEM SHALL still make the target deletions of that `gc` run, and SHALL exit with the code `gc` would have exited with had store eviction found nothing to do. *(Weigh W5)*

## 5. Preview store eviction, and keep scoped runs off the store

**Story:** As a developer or agent, I want `cargo hive gc --dry-run` to show what store eviction would remove without changing the store, so that I can check it before a real run.

- **STOREGC-5.1** WHEN `cargo hive gc --dry-run` runs with no repository argument THE SYSTEM SHALL report the store eviction counts that a real run would report, delete nothing under `<state root>/store/` and change nothing in `store.db`, computing the refresh of STOREGC-1.3 in memory only. *(Weigh A1)*
- **STOREGC-5.2** WHEN no target and no store entry changes between a dry run and the next real run THE SYSTEM SHALL report the same store eviction counts in both. *(Weigh A1)*
- **STOREGC-5.3** WHEN `cargo hive gc <repo>…` runs THE SYSTEM SHALL read and write no file under `<state root>/store/` and no `store.db` content, and add one note saying that store eviction runs only when `gc` has no repository argument. *(Weigh A1)*

## 6. Run gc beside a seed

**Story:** As an agent, I want to run bare `cargo hive gc` while another lane runs `cargo hive seed`, so that neither has to wait for the other and the seeded lane still builds from whole units.

- **STOREGC-6.1** WHILE a `seed` is filling the store or cloning out of it THE SYSTEM SHALL remove no store entry; store eviction tries that exclusion once without waiting, and when a seed holds it, evicts no entry in that run and counts the run as busy. *(agent-decided: the promise; one exclusion for the whole store, amended in design 2026-11-09 from "an entry")*
- **STOREGC-6.2** WHEN a `seed` and a bare `gc` run at the same time THE SYSTEM SHALL leave each unit the seed places in the checkout either with every file its entry holds or with no `.fingerprint/<name-meta>/` directory of it.

## 7. Read what store eviction did

**Story:** As a developer or agent, I want the receipt of bare `cargo hive gc` to count what store eviction removed, kept and freed, so that I can see the store's state without opening it.

- **STOREGC-7.1** WHEN store eviction runs THE SYSTEM SHALL add these deterministic counts to the `gc` receipt, with names fixed in the design: entries evicted by age, by toolchain and by cap; toolchain values dropped; entries kept because their key was held, as a total and by reason (live, Undecided, unreadable name-meta, busy under STOREGC-6.1, whole-store hold); entries left as STOREGC-1.6 describes; freed bytes; bytes still shared; and freeable bytes remaining.
- **STOREGC-7.2** WHEN store eviction removes files THE SYSTEM SHALL count in its freed bytes each removed inode once, only when the removal returned its blocks to the volume, and count the rest of each removed inode's `st_blocks × 512` in its bytes still shared.
- **STOREGC-7.3** IF doctor check (c) has not passed for the active toolchain THEN THE SYSTEM SHALL omit the store's freed bytes, bytes still shared and freeable bytes remaining from the counts, and record each in `unmeasured` with the reason "doctor check (c) did not pass", as `gc` does for target deletions (AUTOGC-5.6).
- **STOREGC-7.4** IF the link count, block count or sharing attributes of a file store eviction removes cannot be read THEN THE SYSTEM SHALL count that file in neither the freed bytes nor the bytes still shared, and add its path to `unmeasured` with the reason.

## 8. Tell agents how the store is bounded

**Story:** As an agent, I want the hive skill to say that bare `cargo hive gc` also bounds the store, so that I keep the store in check with the command I already run.

- **STOREGC-8.1** WHEN an agent reads the hive skill THE SYSTEM SHALL tell it that bare `gc` also evicts from the store by age, toolchain and the 40 GB cap; that `gc --dry-run` previews it; that `gc <repo>` leaves the store alone; which receipt counts report it; and that it must not delete `~/.hive/store/` by hand.
- **STOREGC-8.2** WHEN a person reads `README.md` THE SYSTEM SHALL no longer say that the store only grows until store GC ships, and SHALL say that bare `cargo hive gc` bounds the store; the README sentence `tests/skill.rs` asserts (`tests/skill.rs:569-572`) changes with it.

## 9. Quality attributes

**Section-kind:** nfr

**Story:** As a stakeholder, I want measurable quality targets for this feature, so that how-well is not left implicit.

Consult outcome: no `docs/product/metrics.md`, `docs/ops/reliability.md`, `docs/security/threat-model.md` or `docs/standards/accessibility.md` exists, so the consult is a no-op. The targets below come from the close package (§1 #13, §4) and from ARCH-2 and ARCH-11. No system IDs were invented.

- **Performance:** None, because no latency target was locked for `gc` (AUTOGC story 6). The added cost, liveness over every target plus a toolchain resolution per checkout, is unmeasured on klynt (ledger U1). Its reopen trigger is S2: resolution taking more than 10 s of one run.
- **Security:** **STOREGC-9.1** THE SYSTEM SHALL, in store eviction, only remove entry directories under `<state root>/store/<toolchain value>/`, rename an entry directory into `<state root>/store/.tmp/`, remove an emptied `<state root>/store/<toolchain value>/`, create `<state root>/store.lock`, and write `store.db`; the mechanism is the design's. *(amended in design, 2026-11-09: the store lock of STOREGC-6.1)* This is verified by a test that snapshots a fixture repository and its targets, a seeded checkout, the fixture `CARGO_HOME`, `seeds.json`, `plans.json` and `config.json` before and after a bare `gc` that evicts. It finds every file present with unchanged content and no file added.
- **Reliability:** **STOREGC-9.2** IF `gc` is killed at any point during store eviction THEN THE SYSTEM SHALL leave every store key either with its complete entry directory and its row, or with no directory at its entry path. A partly removed entry never sits at an entry path, where `store::read` would list it or a fill would adopt it. This is verified by a kill-during-eviction test followed by `store::read`, a seed and a fill. *(Weigh W6; the adoption case is from the code-claim review, `src/seed/fill.rs:153-165`)*
- **Reliability:** **STOREGC-9.3** THE SYSTEM SHALL keep the store's freed bytes at or below the increase in the volume's available bytes over that run. This is verified on a dedicated, otherwise idle APFS disk image whose evicted entries include a compressed file and an incompressible file that a seeded checkout still shares by clone.
- **Reliability:** **STOREGC-9.4** WHILE `seed` runs in a loop on new lanes and bare `gc` runs in a loop on the same store THE SYSTEM SHALL produce no seeded unit that STOREGC-6.2 forbids and no `seed` failure caused by eviction. This is verified by a test of at least 12 seed-and-gc cycles. *(agent-decided: mirrors AUTOGC-6.3's 12 cycles)*
- **Accessibility:** None, because this is a headless CLI with JSON output.

## 10. Existing behavior this feature must not change

Files and surfaces this change touches (the module layout is the design's; these are the files it cannot avoid):
- `src/cli.rs`: bare `gc` gains store eviction; `gc <repo>`, `gc --plan`, `apply`, `seed`, `status`, `explain` and `doctor` are at risk. Guarded below.
- `src/seed/store.rs`, `src/seed/place.rs`, `src/seed/fill.rs`, `src/seed/choose.rs`: eviction reads and writes the store; the exclusion of STOREGC-6.1 reaches seed's clone-out and fill's publish. Fill, clone and choose behavior are at risk. Guarded below.
- `src/state.rs`: `store.db` is written through it; its format, `config.json`'s keys and the lock are at risk. Guarded below.
- `src/liveness.rs`: called over every target; its signature, verdicts and purity are at risk. Guarded below.
- `src/doctor.rs`: `resolve_toolchain` is called per checkout; its result and side effects are at risk. Guarded below.
- `src/gc/ledger.rs`, `src/gc/run.rs`: the freeable and freed measurement may reuse the ledger and `measure`; target deletions' counts are at risk. Guarded below.
- `src/discover.rs`, `src/plan_tier/mod.rs`: checkout walks and `faults_that_can_hide_an_owner` are read; ownership and apply are at risk. Guarded below.
- `src/receipt.rs`: no behavior to guard; store eviction only adds counts, findings and notes by name.
- `plugin/skills/hive/SKILL.md`, `tests/skill.rs`: documented commands and their tests. Guarded below.
- `README.md`: its sentence "The store only grows until store GC ships" is asserted by `tests/skill.rs:569-572` and is replaced (STOREGC-8.2). Guarded by STOREGC-10.11.
- `CONTEXT.md`: documentation only; no behavior to guard.

- **STOREGC-10.1** (guard) WHEN bare `cargo hive gc` runs, with or without `--dry-run` THE SYSTEM SHALL CONTINUE TO make the target deletions of AUTOGC stories 2 to 5 and report their counts under the same names, values and exit codes as before this feature.
- **STOREGC-10.2** (guard) WHEN `cargo hive gc <repo>…`, `gc --plan`, `apply`, `status`, `explain`, `doctor`, `repo add` or `repo list` runs THE SYSTEM SHALL CONTINUE TO print the receipt it printed before this feature, with the same schema version, count names, findings and exit codes, apart from the note of STOREGC-5.3.
- **STOREGC-10.3** (guard) WHEN `cargo hive seed` runs and no store eviction runs at the same time THE SYSTEM SHALL CONTINUE TO fill and clone as SEED requires, with the same counts and findings, and SHALL CONTINUE TO evict nothing itself (SEED-2.21).
- **STOREGC-10.4** (guard) WHEN `store::read` or `store::merge` runs THE SYSTEM SHALL CONTINUE TO drop rows whose directory is gone, fill SHALL CONTINUE TO adopt a directory that has no row when it meets that key live, and a published entry directory SHALL CONTINUE TO be modified by nothing; store eviction only removes one whole (SEED design decision 6, whose "never touched again" this narrows to "never modified").
- **STOREGC-10.5** (guard) WHEN hive writes `store.db`, `seeds.json` or `plans.json` THE SYSTEM SHALL CONTINUE TO use the same fields as before this feature, written through the `state` module under its one lock, and WHEN it reads `config.json` it SHALL CONTINUE TO accept the same keys and to write nothing to it.
- **STOREGC-10.6** (guard) WHEN `liveness()` runs on any target THE SYSTEM SHALL CONTINUE TO return the verdicts of LIVE stories 1 and 2, with its signature unchanged and no state written.
- **STOREGC-10.7** (guard) WHEN `seed` or `doctor` calls `resolve_toolchain` THE SYSTEM SHALL CONTINUE TO satisfy CORE-1.11 and CORE-1.12 and install no toolchain (`RUSTUP_AUTO_INSTALL=0`).
- **STOREGC-10.8** (guard) WHEN `seed` looks up a checkout's `-vV` text THE SYSTEM SHALL CONTINUE TO report `toolchain.missing` when the table has no row for that whole text, and `units.empty` when the chosen set is empty.
- **STOREGC-10.9** (guard) WHEN the ledger measures a file deleted from a target THE SYSTEM SHALL CONTINUE TO count it as AUTOGC-5.1, AUTOGC-5.2 and SEED-5.1 require.
- **STOREGC-10.10** (guard) WHEN `apply` meets a checkout-walk fault or a target-scan fault THE SYSTEM SHALL CONTINUE TO refuse or proceed as `split_checkout_faults` and `faults_that_can_hide_an_owner` decide today.
- **STOREGC-10.11** (guard) WHEN the skill tests run THE SYSTEM SHALL CONTINUE TO require that the skill body stays within its line budget, that it keeps its seven fixed headings in order and names no local `.md` file, that every command line in it parses with the real CLI and carries `--json`, that `metadata.hive-version` equals the crate version, and that the README sentences it asserts are present, the one STOREGC-8.2 replaces updated with it.
- **STOREGC-10.12** (guard) WHEN this feature is built THE SYSTEM SHALL CONTINUE TO depend on nothing beyond its existing crates unless the owner approves one.
- **STOREGC-10.13** (guard) WHEN `seed` reports `store_bytes` THE SYSTEM SHALL CONTINUE TO measure the allocated bytes of the whole store directory, `.tmp` included, as it does today.

## Out of Scope

- A store plan class under `gc --plan` / `apply`: a new `PlanClass` variant is a persisted-format change (charter hard stop).
- A user-set cap or max age in this run: a `config.json` key is a persisted-format change. This is a deferral, not a product non-goal; `docs/product/vision.md:71` lists both beside defaults that are `config.json` keys, and it reopens under `--go`.
- Any new field in `store.db`, a clone id included.
- Store eviction inside `seed`, a throttle, or a new subcommand.
- Deleting anything inside a Cargo target dir or a checkout as part of store eviction.
- Evicting a held key, or part of an entry.
- A daemon, wrapper or hook.
- Cold-versus-seeded timing: ROAD-11.
- Rewriting the hive skill as one guide: ROAD-13.
- Fill, clone-out and choose changes beyond what STOREGC-6.1 and STOREGC-6.2 need: ROAD-8, built.

## Open Questions

- Whether the ledger's rule measures a compressed store file correctly, that is, what a compressed store file no checkout shares reads for `shares_all` (ledger S4, K4) — design-solution — 2026-11-09 — forbid-guess: measure it on APFS before STOREGC-3.1 and STOREGC-7.2 are designed.
- Whether an entry whose `registry_deps` name an evicted entry counts as evictable at once or waits for the age rule (ledger S5, K6) — design-solution — 2026-11-09 — forbid-guess.
- SEED's design text for decision 6 ("never touched again") and its `store_bytes` count need a note once eviction exists (code-claim review item 8) — design-solution — 2026-11-09 — forbid-guess.
- `seed`'s per-checkout resolution keeps a `default`-source `RUSTUP_TOOLCHAIN`, which beats a checkout's `rust-toolchain.toml` (verified on rustup 1.29.3, ledger V1, S6), against SEED lock 6 — owner, through root-cause, outside this run — 2026-11-09 — forbid-guess.
- The cost of toolchain resolution per checkout on klynt (ledger U1, S2) — design-solution — 2026-11-09 — forbid-guess: measure, do not estimate.
