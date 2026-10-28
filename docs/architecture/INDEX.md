# Architecture: cargo-hive

Status: Approved
Date: 2026-10-28

<!--
The architecture spine: the small set of invariants that keep independently built
features consistent. Each is a bold ARCH-N ID plus one imperative rule. Feature
design.md files cite the ones they rely on as `Respects: ARCH-N`; the `audit-trace`
check verifies those citations point at a live invariant, and `inspect-invariants`
judges whether a design or diff actually conforms.

Rules:
- ARCH-N is flat and repo-wide: unique forever, never renumbered, never reused.
- Retire an invariant by strikethrough with the ID that supersedes it, in prose only.
- Counting methods, lock primitives and other mechanisms belong in feature designs;
  an invariant states the promise they must keep.

Source: the define-project interview close package confirmed 2026-10-28
(.skills/_pending-define-project/close-package.md), which draws on the CORE close
package (A1-A6, D2-D5, D7, S1-S2, C1), the F9 amendment and SPIKES.md.
-->

## Invariants

- **ARCH-1** Stay out of the build path: install no rustc wrapper, cargo shim, Cargo config, environment variable or hook.
- **ARCH-2** Write outside hive's state root and doctor's temp dir only by deleting from, or cloning into, a Cargo target dir; never write a repo file or `~/.cargo`.
- **ARCH-3** Never replace a build artifact in place.
- **ARCH-4** On a protected checkout, only read or delete.
- **ARCH-5** Seed only secondary checkouts.
- **ARCH-6** Compress only in the store, then clone out; never compress a clone.
- **ARCH-7** Classify every checkout by the C1 rules, and treat anything unclassifiable as protected.
- **ARCH-8** Take operation scope from arguments or a plan, never from the current directory.
- **ARCH-9** Accept Cargo fingerprint files only in a known exact format signature; refuse to act on any other shape.
- **ARCH-10** Report only deterministic counters measured on this machine; seconds come only from `cargo hive bench`.
- **ARCH-11** Report as freed only the bytes a deletion returns to the volume; bytes a surviving hardlink or clone still holds are counted apart, never as freed.
- **ARCH-12** Open an owner-authentication dialog only from `cargo hive approve`, with a TTY and no agent ancestor.
- **ARCH-13** A child `git` or `cargo` never inherits the caller's target-dir or `GIT_*` variables, and a toolchain selected by the caller's directory never leaks into hive's choice of active toolchain; every other variable the child sees is set by hive or kept on purpose.
- **ARCH-14** Gate each capability on `cargo hive doctor` results for the active toolchain.
- **ARCH-15** Keep macOS-specific code (anything under `cfg(target_os = "macos")`, APFS and LocalAuthentication) behind the `src/platform` `Fs` and `Approve` traits.
- **ARCH-16** Write hive's metadata files (registry, capabilities, snapshot, plans, `store.db`) only through the `state` module, atomically, under its one lock.
- **ARCH-17** Never compile or replay a build to decide liveness or ownership; ownership comes only from `cargo metadata --no-deps --locked --offline`.
- **ARCH-18** A deletion inside a Cargo target dir never removes a fingerprint dir on its own and never touches `build/`; only a plan that deletes a whole orphan target removes the target entire.
- **ARCH-19** Never delete from, or fill out of, a Cargo target dir while another process holds the lock guarding that entry; the lock is tried without waiting and the entry is skipped when held.
- **ARCH-20** A plan is applied only after rechecking liveness, locks and format at apply time; a stale plan is refused.

## Domains

None. Every invariant lives in this file.
