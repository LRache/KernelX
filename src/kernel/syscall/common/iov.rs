use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::syscall::UserStruct;
use crate::kernel::syscall::uptr::UPtr;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct IOVec {
    pub base: usize,
    pub len: usize,
}

impl UserStruct for IOVec {}

#[derive(Clone, Copy)]
pub struct IOVecCursor {
    uptr_iov: UPtr<IOVec>,
    iovcnt: usize,
    index: usize,
    iov: IOVec,
    offset: usize,
}

impl IOVecCursor {
    pub fn new(uptr_iov: UPtr<IOVec>, iovcnt: usize) -> Self {
        Self {
            uptr_iov,
            iovcnt,
            index: 0,
            iov: IOVec { base: 0, len: 0 },
            offset: 0,
        }
    }

    fn load_next(&mut self) -> SysResult<bool> {
        while self.index < self.iovcnt {
            let iov = self.uptr_iov.add(self.index).read()?;
            self.index += 1;

            if (iov.len as isize) < 0 {
                return Err(Errno::EINVAL);
            }
            if iov.len == 0 {
                continue;
            }
            iov.base.checked_add(iov.len - 1).ok_or(Errno::EFAULT)?;

            self.iov = iov;
            self.offset = 0;
            return Ok(true);
        }

        Ok(false)
    }

    pub fn remaining(&mut self) -> SysResult<Option<usize>> {
        while self.offset == self.iov.len {
            if !self.load_next()? {
                return Ok(None);
            }
        }

        Ok(Some(self.iov.len - self.offset))
    }

    pub fn current_addr(&self) -> Option<usize> {
        self.iov.base.checked_add(self.offset)
    }

    pub fn advance(&mut self, bytes: usize) {
        self.offset += bytes;
    }
}
