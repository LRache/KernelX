use crate::kernel::errno::{Errno, SysResult};
use crate::driver;
use crate::fs::file::DirResult;
use crate::fs::{InodeOps, FileType, Mode};

use super::def::*;
use super::superblock::DevFileSystem;

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
        match index {
            0 => return Ok(Some((DirResult { ino: ROOT_INO, name: ".".into(), file_type: FileType::Directory}, index + 1))),
            1 => return Ok(Some((DirResult { ino: NULL_INO, name: "null".into(), file_type: FileType::Regular}, index + 1))),
            2 => return Ok(Some((DirResult { ino: ZERO_INO, name: "zero".into(), file_type: FileType::Regular}, index + 1))),
            _ => {}
        };

        let driver_ids = driver::driver_ids().read();
        let index = index - 2;
        if index < driver_ids.len() {
            let (_, driver) = driver_ids.iter().nth(index).unwrap();
            let dent = DirResult {
                ino: 3 + index as u32,
                name: driver.device_name(),
                file_type: match driver.device_type() {
                    driver::DeviceType::Block => FileType::BlockDevice,
                    driver::DeviceType::Char => FileType::CharDevice,
                    driver::DeviceType::Rtc => FileType::CharDevice,
                },
            };
            return Ok(Some((dent, index + 3)));
        } else {
            Ok(None)
        }      
    }

    fn lookup(&self, name: &str) -> SysResult<u32> {
        let r = match name {
            "null" => NULL_INO,
            "zero" => ZERO_INO,
            _ => return Err(Errno::ENOENT),
        };

        Ok(r)
    }

    fn mode(&self) -> SysResult<Mode> {
        Ok(Mode::from_bits(Mode::S_IFDIR.bits() | 0o755).unwrap())
    }

    fn size(&self) -> SysResult<u64> {
        Ok(core::mem::size_of::<DevFileSystem>() as u64)
    }
}
