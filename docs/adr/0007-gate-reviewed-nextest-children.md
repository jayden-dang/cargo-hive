# Gate reviewed Nextest children

The pinned Cargo builds forward an explicit `CARGO` when both the current
executable and argv zero use a private basename. Native fixture probes on both
pins confirmed that a byte-identical copy reaches stock Nextest with the same
arguments and reaches a checked metadata gate from an actual SQLx expansion.

The approved exception applies to the reviewed Nextest workspace and gateways
recipes and both arms of their timing pairs. The gate accepts the reviewed
Cargo metadata and test-build commands, binds compiler artifact messages to
reviewed listing units and checks the binary, package directory and listing
arguments at the runner. SQLx metadata checks bind the reviewed consumer and
workspace to the offline operation. Compilation, startup and generated-input
reviews remain prerequisites for a real-workspace permit.
