use crate::kernel::errno::{Errno, SysResult};
use crate::fs::file::DirResult;
use crate::fs::{InodeOps, FileType};

use super::def::*;

pub struct RootInode {
    sno: u32,
}

impl RootInode {
    pub fn new(sno: u32) -> Self {
        Self { sno }
    }
}

impl InodeOps for RootInode {
    fn get_ino(&self) -> u32 {
        ROOT_INO
    }

    fn get_sno(&self) -> u32 {
        self.sno
    }

    fn type_name(&self) -> &'static str {
        "devfs"
    }

    fn get_dent(&self, index: usize) -> SysResult<Option<(DirResult, usize)>> {
        let r = match index {
            // 0 => DirResult { ino: ROOT_INO, name: ".".into(), file_type: FileType::Directory, len: 1},
            // 1 => DirResult { ino: NULL_INO, name: "null".into(), file_type: FileType::Regular, len: 1 },
            // 2 => DirResult { ino: ZERO_INO, name: "zero".into(), file_type: FileType::Regular, len: 1 },
            0 => DirResult { ino: ROOT_INO, name: ".".into(), file_type: FileType::Directory},
            1 => DirResult { ino: NULL_INO, name: "null".into(), file_type: FileType::Regular},
            2 => DirResult { ino: ZERO_INO, name: "zero".into(), file_type: FileType::Regular},
            _ => return Ok(None),
        };

        Ok(Some((r, index + 1)))
    }

    fn lookup(&self, name: &str) -> SysResult<u32> {
        let r = match name {
            "null" => NULL_INO,
            "zero" => ZERO_INO,
            _ => return Err(Errno::ENOENT),
        };

        Ok(r)
    }
}
