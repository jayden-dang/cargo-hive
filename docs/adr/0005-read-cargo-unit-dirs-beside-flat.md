# ADR-0005: Read Cargo unit dirs beside the flat fingerprint layout

Status: Accepted (2026-11-17, with BUILDDIR Decision 7)

Cargo 1.100 stabilised the build-dir layout (cargo issue 15010): each unit lives under
`build/<package>/<hash>/` with its own `fingerprint/`, `out/` and optional `run/`, and a profile
dir may hold that layout alone or mixed with the flat `.fingerprint/` layout. ADR-0001's note that
the format "already differs on nightly" is now the stable case. Hive keeps one reader for both
layouts, one graph over them, `UnitLoc` and `unit_path` as the only home for a unit's paths, and
deletes a dead unit-dir unit as its whole dir with the fingerprint last (ARCH-22).

Rejected alternatives:
- **Flat-looking `dir_name`s.** Pretending a unit-dir unit is a `.fingerprint/<name>-<hash>` string
  hides the layout and forces every caller to re-derive paths.
- **A separate 1.100 code path.** Two readers and two graphs would diverge on mixed profile dirs and
  on every later layout change.
- **Refusing mixed profile dirs.** Building with 1.99 then 1.100 in one target produces them; hive
  already has to read both.
- **Pinning users to 1.99.** The unit-dir layout is stable; hive follows Cargo.

Revisit when Cargo changes either layout again.
