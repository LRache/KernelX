#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(dead_code)]

include!(concat!(env!("OUT_DIR"), "/ext4_bindings.rs"));

unsafe impl Send for ext4_inode_ref {}
unsafe impl Sync for ext4_inode_ref {}
