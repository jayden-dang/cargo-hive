//! Plan-tier removal inside one directory: a path is deleted only after its canonical
//! parent is shown to lie inside the root and its directory entry is the one expected.
//! Removal does not follow a symbolic link. Canonicalizing the parent is what stops a
//! link from pointing that removal outside the root.

pub mod guard;
