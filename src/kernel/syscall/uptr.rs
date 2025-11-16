use alloc::string::String;

use crate::kernel::mm::uptr::{self, UserPointer};
use crate::kernel::errno::SysResult;
use crate::kernel::scheduler::current;

pub struct UPtr<T: Copy> {
    inner: uptr::UPtr<T>,
}

impl<T: Copy> UPtr<T> {
    pub fn should_not_null(&self) -> SysResult<()> {
        self.inner.should_not_null()
    }

    pub fn uaddr(&self) -> usize {
        self.inner.uaddr()
    }

    pub fn kaddr(&self) -> SysResult<usize> {
        self.inner.kaddr()
    }

    pub fn is_null(&self) -> bool {
        self.inner.is_null()
    }

    pub fn read(&self) -> SysResult<T> {
        self.inner.read(current::addrspace())
    }

    pub fn write(&self, value: T) -> SysResult<()> {
        self.inner.write(value, current::addrspace())
    }

    pub fn read_optional(&self) -> SysResult<Option<T>> {
        if self.is_null() {
            Ok(None)
        } else {
            Ok(Some(self.read()?))
        }
    }

    pub fn add(&self, offset: usize) -> Self {
        Self {
            inner: self.inner.add(offset),
        }
    }
}

impl<T: Copy> From<usize> for UPtr<T> {
    fn from(uaddr: usize) -> Self {
        UPtr {
            inner: uptr::UPtr::from(uaddr),
        }
    }
}

pub struct UArray<T: Copy> {
    inner: uptr::UArray<T>,
}

impl<T: Copy> UArray<T> {
    pub fn uaddr(&self) -> usize {
        self.inner.uaddr()
    }

    pub fn is_null(&self) -> bool {
        self.inner.is_null()
    }

    pub fn should_not_null(&self) -> SysResult<()> {
        self.inner.should_not_null()
    }

    pub fn read(&self, offset: usize, buf: &mut [T]) -> SysResult<()> {
        self.inner.read(offset, buf, current::addrspace())
    }

    pub fn write(&self, offset: usize, buf: &[T]) -> SysResult<()> {
        self.inner.write(offset, buf, current::addrspace())
    }

    pub fn index(&self, i: usize) -> UPtr<T> {
        UPtr {
            inner: self.inner.index(i),
        }
    }
}

impl<T: Copy> From<usize> for UArray<T> {
    fn from(uaddr: usize) -> Self {
        UArray {
            inner: uptr::UArray::from(uaddr),
        }
    }
}

pub type UBuffer = UArray<u8>;

#[derive(Clone, Copy)]
pub struct UString {
    inner: uptr::UString,
}

impl UString {
    pub fn is_null(&self) -> bool {
        self.inner.is_null()
    }

    pub fn should_not_null(&self) -> SysResult<()> {
        self.inner.should_not_null()
    }

    pub fn read(&self) -> SysResult<String> {
        self.inner.read()
    }

    pub fn write(&self, s: &str, max_size: usize) -> SysResult<usize> {
        self.inner.write(s, max_size)
    }
}

impl From<usize> for UString {
    fn from(uaddr: usize) -> Self {
        UString {
            inner: uptr::UString::from(uaddr),
        }
    }
}
