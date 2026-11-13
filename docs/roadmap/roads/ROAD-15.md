# ROAD-15 workspace-split

**Outcome:** Hive's code lives in several crates, each with one job, that depend only on the
layers below them, so a contributor or agent can change one part and build and test only
what it touches, while `cargo install cargo-hive` still installs the same commands.

**Examples:**
1. Jayden changes how gc decides a dead unit: the gc crate and the binary rebuild, and the
   seed crate does not.
2. An agent adds a direct OS call in the seed crate: the build fails and names the rule that
   only the platform crate may touch the OS.
3. After the release, Jayden runs `cargo install cargo-hive` from crates.io and gets the same
   commands, flags and receipt fields, and klynt's 0.1.0 state still reads.
4. On klynt's main target, liveness after the rework finds the same live and dead units as
   0.1.0 does on that target.

**Must keep:** every command, flag, receipt field, exit code and state file as 0.1.0 has
them; one version shared by every crate and the plugin; macOS only.
**Off limits:** new commands or behaviour; Linux; changing state file layouts (ROAD-14).
**Decided:** done before ROAD-14; every crate is published to crates.io; the rework of data
structures and the move to edition 2024 are part of this road; it sits in MILE-4; the build
that ignores edits after packaging is root-caused and fixed first.
**Delegated:** where each module goes; which data structures change and how, when results
stay equal; test layout.
**Ask me:** crate names, before anything is published; any change a user can see in output
or receipts; a new third-party dependency.
**UI:** none
