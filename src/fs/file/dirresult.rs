use alloc::string::String;

use crate::fs::FileType;

#[derive(Clone)]
pub struct DirResult {
    pub name: String,
    pub ino: u32,
    pub file_type: FileType,
    pub len: u16,
}
