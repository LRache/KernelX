use core::fmt::Debug;

use crate::arch;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::mm::AddrSpace;

pub struct UAddrSpaceBuffer<'a> {
    uaddr: usize,
    length: usize,
    addrspace: &'a AddrSpace,
}

impl<'a> UAddrSpaceBuffer<'a> {
    pub fn new(uaddr: usize, length: usize, addrspace: &'a AddrSpace) -> Self {
        Self {
            uaddr,
            length,
            addrspace,
        }
    }

    pub fn write(&self, offset: usize, buf: &[u8]) -> SysResult<()> {
        debug_assert!(offset + buf.len() <= self.length);
        self.addrspace.copy_to_user_buffer(self.uaddr + offset, buf)
    }

    pub fn length(&self) -> usize {
        self.length
    }

    pub fn iter(&'a self) -> ReaderIter<'a> {
        ReaderIter { ubuf: self, offset: 0 }
    }

    pub fn iter_mut(&'a self) -> WriterIter<'a> {
        WriterIter { ubuf: self, offset: 0 }
    }
}

pub struct ReaderIter<'a> {
    ubuf: &'a UAddrSpaceBuffer<'a>,
    offset: usize,
}

impl Iterator for ReaderIter<'_> {
    type Item = SysResult<&'static [u8]>;

    fn next(&mut self) -> Option<Self::Item> {
        let remaining = self.ubuf.length - self.offset;
        if remaining == 0 {
            return None;
        }

        let uaddr = self.ubuf.uaddr + self.offset;
        let kaddr = match self
            .ubuf
            .addrspace
            .with_map_manager_mut(|manager| manager.translate_read(uaddr, self.ubuf.addrspace))
        {
            Some(kaddr) => kaddr,
            None => return Some(Err(Errno::EFAULT)),
        };

        let length = core::cmp::min(remaining, arch::PGSIZE - (kaddr % arch::PGSIZE));
        self.offset += length;

        debug_assert!(kaddr % arch::PGSIZE + length <= arch::PGSIZE);
        debug_assert!(kaddr != 0);

        Some(Ok(unsafe { core::slice::from_raw_parts(kaddr as *const u8, length) }))
    }
}

pub struct WriterIter<'a> {
    ubuf: &'a UAddrSpaceBuffer<'a>,
    offset: usize,
}

impl Iterator for WriterIter<'_> {
    type Item = SysResult<&'static mut [u8]>;

    fn next(&mut self) -> Option<Self::Item> {
        let remaining = self.ubuf.length - self.offset;
        if remaining == 0 {
            return None;
        }

        let uaddr = self.ubuf.uaddr + self.offset;
        let kaddr = match self
            .ubuf
            .addrspace
            .with_map_manager_mut(|manager| manager.translate_write(uaddr, self.ubuf.addrspace))
        {
            Some(kaddr) => kaddr,
            None => return Some(Err(Errno::EFAULT)),
        };

        let length = core::cmp::min(remaining, arch::PGSIZE - (kaddr % arch::PGSIZE));
        self.offset += length;

        debug_assert!(kaddr % arch::PGSIZE + length <= arch::PGSIZE);
        debug_assert!(kaddr != 0);

        Some(Ok(unsafe { core::slice::from_raw_parts_mut(kaddr as *mut u8, length) }))
    }
}

impl Debug for UAddrSpaceBuffer<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("UAddrSpaceBuffer")
            .field("uaddr", &self.uaddr)
            .field("length", &self.length)
            .finish()
    }
}
