# Architecture: cargo-hive

Status: Approved
Date: 2026-11-02

<!--
The architecture spine: the small set of invariants that keep independently built
features consistent. Each is a bold ARCH-N ID plus one imperative rule. Feature
design.md files cite the ones they rely on as `Respects: ARCH-N`; the `audit-trace`
check verifies those citations point at a live invariant, and `inspect-invariants`
judges whether a design or diff actually conforms.

Rules:
- ARCH-N is flat and repo-wide: unique forever, never renumbered, never reused.
- Reword an invariant in place under the same ID while its promise is unchanged.
- Retire an invariant, only when its promise changes or dies, by strikethrough, naming the ID that supersedes it or stating there is none (as ARCH-12 does), in prose only.
- Counting methods, lock primitives and other mechanisms belong in feature designs;
  an invariant states the promise they must keep.

Source: the define-project interview close package confirmed 2026-10-28
(.skills/_pending-define-project/close-package.md), which draws on the CORE close
package (A1-A6, D2-D5, D7, S1-S2, C1), the F9 amendment and SPIKES.md.
-->

## Invariants

- **ARCH-1** Stay out of the build path: install no rustc wrapper, cargo shim, Cargo config, environment variable or hook.
- **ARCH-2** Write outside hive's state root and doctor's temp dir only by deleting from, or cloning into, a Cargo target dir, or by creating directories inside that target dir that those clones need; never change repo file contents or write `~/.cargo`. A slot verifier may launch the guide's fixed timestamp action with a deadline on a verified regular consumer-anchor descriptor under the current preparation fence.
- **ARCH-3** Never replace a build artifact in place.
- **ARCH-4** On a protected checkout, only read or delete.
- **ARCH-5** Seed only secondary checkouts.
- ~~**ARCH-6**~~ retired 2026-11-17: the store no longer compresses its files, because rustc rejects an rmeta that keeps the compressed-file flag, so placed clones had to be expanded (SEED-2.28 to SEED-2.30). Superseded by ARCH-21.
- **ARCH-7** Classify every checkout by the C1 rules, and treat anything unclassifiable as protected.
- **ARCH-8** Take operation scope from arguments or a plan, never from the current directory.
- **ARCH-9** Accept Cargo fingerprint files only in a known exact format signature; refuse to act on any other shape.
- **ARCH-10** Report only deterministic counters measured on this machine; durations come only from `cargo hive bench`, as integer milliseconds. The rule binds what hive reports and claims. A document a person writes about their own measurements, such as `docs/scorecard.md`, may include a duration timed by hand when it is labelled as outside bench, written in integer milliseconds with the machine and toolchain, and never appears in a hive receipt or finding (clarified 2026-11-23).
- **ARCH-11** Report as freed only the bytes a deletion returns to the volume; bytes a surviving hardlink or clone still holds are counted apart, never as freed.
- ~~**ARCH-12** Open an owner-authentication dialog only from `cargo hive approve`, with a TTY and no agent ancestor.~~ Retired 2026-11-02, with no successor: hive has no dialog and no approval command (ADR-0002).
- **ARCH-13** A child `git` or `cargo` never inherits the caller's target-dir or `GIT_*` variables, and a toolchain selected by the caller's directory never leaks into hive's choice of active toolchain; every other variable the child sees is set by hive or kept on purpose.
- **ARCH-14** Gate each capability on `cargo hive doctor` results for the active toolchain.
- **ARCH-15** Keep macOS-specific code (anything under `cfg(target_os = "macos")`, APFS) behind the `crates/platform` `Fs` trait.
- **ARCH-16** Write hive's metadata files (registry, capabilities, snapshot, plans, `store.db`) only through the `state` module, atomically, under its one lock.
- **ARCH-17** Never compile or replay a build to decide liveness or ownership; ownership comes only from `cargo metadata --no-deps --locked --offline`.
- ~~**ARCH-18**~~ retired 2026-11-17: it forbade any touch of `build/` and any lone fingerprint removal, which cannot hold for Cargo 1.100 unit dirs. Superseded by ARCH-22.
- **ARCH-19** Never delete from, or fill out of, a Cargo target dir while another process holds the lock guarding that entry; the lock is tried without waiting and the entry is skipped when held.
- **ARCH-20** A plan is applied only after rechecking liveness, locks and format at apply time; a stale plan is refused.
- **ARCH-21** Never compress a store file; a clone placed from the store shares every block with its store entry.
- **ARCH-22** A deletion inside a Cargo target dir removes a unit's fingerprint only in the same run as all of that unit's outputs and after them. In the flat layout these are its hash-keyed `deps/` and `examples/` files, never anything under `build/`. In the unit-dir layout they are its whole `build/<package>/<hash>/` dir. Build-script units are never deleted, and only a plan that deletes a whole orphan target removes the target entire.

## Domains

None. Every invariant lives in this file.
