# ROAD-14 state-upgrade-safety

**Outcome:** A user or agent who upgrades hive keeps every registered repo, plan, receipt and
store entry an earlier release wrote, or is stopped with the file named and the command that
fixes it; hive never starts empty because it could not read what is there.

**Examples:**
1. Jayden upgrades from 0.1.0 to the next release: `cargo hive status` lists klynt and its
   lanes as before, and the next `seed` clones the store's 596 registry units with no refill.
2. A registry written by 0.1.0 has a shape the new release cannot read: every command stops,
   names the registry file and the command to run, and leaves the file untouched.
3. An agent in a build-in-waves lane runs `cargo hive gc --json` right after the upgrade and
   gets the same receipt shape it got before, with the release's own version in it.
4. The store index cannot be read: hive refuses to seed and says why, instead of filling the
   store again from scratch.

**Must keep:** a first run with no hive state works with zero configuration; a file hive
cannot read is refused and left untouched, as today.
**Off limits:** downgrades to an older hive; Cargo and rustc file formats, which doctor gates.
**Decided:** lands before the next release; every state file 0.1.0 writes has a test fixture
taken from 0.1.0; a file that exists but cannot be read is never treated as empty.
**Delegated:** whether each old layout is read as is, converted, or refused; how a file
records its layout.
**Ask me:** any upgrade path that makes a user delete or rebuild hive's state.
**UI:** none
