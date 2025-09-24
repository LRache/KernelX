use alloc::string::String;

use crate::fs::file::file::FileType;

#[derive(Clone)]
pub struct DirResult {
    pub name: String,
    pub ino: u32,
    pub file_type: FileType,
}
