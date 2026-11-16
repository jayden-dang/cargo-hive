//! Shared caps for directory listings and Mach-O debug-map reads.

/// Most entries listed from one `incremental/` level, crate dir or scanned directory.
pub const CRATE_DIR_CAP: usize = 100_000;
/// Most entries listed from one `deps/` directory.
pub const DEPS_CAP: usize = 1_000_000;
/// Most bytes of Mach-O load commands (`sizeofcmds`) a debug-map read will accept.
pub const MACHO_CMDS_CAP: u64 = 4 << 20;
/// Most bytes of Mach-O symbol table (`nsyms × 16`) a debug-map read will accept.
pub const MACHO_SYMTAB_CAP: u64 = 256 << 20;
/// Most bytes of Mach-O string table (`strsize`) a debug-map read will accept.
pub const MACHO_STRTAB_CAP: u64 = 512 << 20;
/// Most architectures in a fat Mach-O a debug-map read will accept.
pub const FAT_ARCH_CAP: u32 = 8;
