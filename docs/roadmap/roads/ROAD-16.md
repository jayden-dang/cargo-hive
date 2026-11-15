# ROAD-16 checkout-gc-debug-objects

**Outcome:** An agent or developer runs gc on one checkout and gets back the speed of a fresh
target: units no build still uses, and debug objects no linked binary references, are removed
from that checkout only.

**Examples:**
1. Jayden's aged slot in klynt holds 121k files in `deps/`, and an edit's test build takes
   60 s. After gc on that slot, `deps/` holds 4.2k files and the same build takes 28 s.
2. An agent runs gc on its slot while Jayden builds in klynt's main checkout: only the slot is
   touched; the main target and every lane keep all their files.
3. A build holds the slot's lock when gc starts: hive skips the slot, says so in the receipt,
   and deletes nothing there.
4. A test binary the task used but never recompiled keeps its units: the next task
   recompiles none of them because of the gc.

**Must keep:** no unit the last task used rebuilds because of a gc; today's repo-wide `gc`,
`gc --plan` and `apply` behave as they do now.
**Off limits:** hooks, wrappers, background processes, Cargo config; deleting anything outside
the named checkout; Linux.
**Decided:** an explicit command, ARCH-1 stays; it is proven on klynt first. One command deletes directly, with no plan step; it may target any registered checkout, secondary or protected. Dead units keep today's whole-day window; a tighter window waits for #20 and a measurement (2026-11-15).
**Delegated:** how referenced debug objects are found and cached; the `deps/` size at which
the skill calls gc.
**Ask me:** anything that needs a hook, daemon or wrapper; deleting a file a live binary
references.
**UI:** none
