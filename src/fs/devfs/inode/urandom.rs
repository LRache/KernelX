use alloc::sync::Arc;

use num_enum::TryFromPrimitive;

use crate::fs::file::{FileFlags, FileOps, RandomAccessFile};
use crate::fs::{Dentry, Inode, InodeOps, Mode};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::mm::AddrSpace;
use crate::kernel::uapi::FileStat;
use crate::klib::random;

#[repr(i32)]
enum Entropy {
    Available = 256,
}

pub struct URandomInode {
    ino: u32,
}

impl URandomInode {
    pub fn new(ino: u32) -> Self {
        Self { ino }
    }
}

impl InodeOps for URandomInode {
    fn get_ino(&self) -> u32 {
        self.ino
    }

    fn type_name(&self) -> &'static str {
        "devfs"
    }

    fn readat(&self, buf: &mut [u8], _offset: usize, _direct: bool) -> SysResult<usize> {
        buf.iter_mut().for_each(|b| {
            *b = (random::random() & 0xFF) as u8;
        });
        Ok(buf.len())
    }

    fn writeat(&self, buf: &[u8], _offset: usize) -> SysResult<usize> {
        Ok(buf.len())
    }

    fn ioctl(&self, request: usize, arg: usize, addrspace: &AddrSpace) -> SysResult<usize> {
        #[derive(TryFromPrimitive)]
        #[allow(non_camel_case_types)]
        #[repr(u32)]
        enum Request {
            RNDGETENTCNT = 0x80045200,
        }

        let request = Request::try_from_primitive(request as u32).map_err(|_| Errno::ENOTTY)?;
        match request {
            Request::RNDGETENTCNT => {
                addrspace.copy_to_user(arg, Entropy::Available as i32)?;
                Ok(0)
            }
        }
    }

    fn size(&self) -> SysResult<u64> {
        Ok(0)
    }

    fn fstat(&self) -> SysResult<FileStat> {
        let mut kstat = FileStat::default();
        kstat.st_ino = self.ino as u64;
        kstat.st_size = 0;
        kstat.st_mode = self.mode()?.bits() as u32;
        kstat.st_nlink = 1;
        kstat.st_gid = 0;
        kstat.st_uid = 0;
        Ok(kstat)
    }

    fn mode(&self) -> SysResult<Mode> {
        Ok(Mode::from_bits_truncate(Mode::S_IFCHR.bits() as u32 | 0o666))
    }

    fn wrap_file(&self, inode: Arc<Inode>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps> {
        Arc::new(RandomAccessFile::new(inode, dentry.unwrap(), flags))
    }
}
