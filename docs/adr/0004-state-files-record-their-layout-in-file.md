# ADR-0004: Each state file records its layout as its own first key

Status: Proposed (2026-11-15, unattended ROAD-14 run; the operator decides at the spec sheet)

Hive's JSON state files record no layout today, and ROAD-14 needs each to record one (ticket #13, UPGRADE-1.1).
Each of the six persisted types (`Registry`, `Capabilities`, `Snapshot`, `PlansFile`, `StoreFile`, `SeedsFile`)
therefore declares `layout: cargo_hive_state::Layout` as its first field. A file without the key reads as layout
1, a value frozen forever (`#[serde(default = "Layout::absent")]`), while a new file is written at
`Layout::NEWEST`. The state module reads the key before the strict parse, so a file from a newer hive is
refused as such rather than as malformed.

Rejected alternatives:
- **A generic `Layered<T>` wrapper with `#[serde(flatten)]`.** serde does not support `flatten` together with
  `deny_unknown_fields` on either struct (serde.rs container-attrs and attr-flatten), and three of the types use
  it.
- **Splicing the key into the bytes in `encode`.** It is invisible to the types and fragile.
- **A root manifest.** Its two files cannot be written atomically, and it adds a file to every home.
- **Versioned file names.** A rename reads as a missing file, which reads as empty.

Accepted costs:
- The 0.1.x readers with `deny_unknown_fields` refuse a file once this release writes it (downgrades are off
  limits).
- Every struct literal of these types gains one field.
- SPLIT-4.3 is superseded by UPGRADE-2.8.

Revisit if a seventh state file is added without the field, or if one file needs a layout the others do not
share.
