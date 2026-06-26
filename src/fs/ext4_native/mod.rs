// Experimental read-only native ext4 backend. On-disk layout parsing and extent
// traversal are informed by ../ext4_rs (MIT) and adapted for KernelX VFS.

mod filesystem;
mod inode;
mod superblock;
mod utils;

pub use filesystem::FileSystem;
