use alloc::string::String;
use core::mem::size_of;

use crate::kernel::scheduler::current::{copy_from_user, copy_to_user};
use crate::kernel::errno::{SysResult, Errno};

/// Macro to implement From<usize> for user pointer types
macro_rules! impl_from_usize {
    ($type_name:ident<T>) => {
        impl<T: Copy> From<usize> for $type_name<T> {
            fn from(uaddr: usize) -> Self {
                $type_name {
                    uaddr,
                    _marker: core::marker::PhantomData,
                }
            }
        }
    };
}

pub trait UserPointer<T: Copy> {
    fn from_uaddr(uaddr: usize) -> Self;

    fn uaddr(&self) -> usize;
    
    fn is_null(&self) -> bool {
        self.uaddr() == 0
    }
    
    fn should_not_null(&self) -> SysResult<()> {
        if self.is_null() {
            Err(Errno::EINVAL)
        } else {
            Ok(())
        }
    }

    fn index(&self, i: usize) -> Self where Self: Sized {
        Self::from_uaddr(self.uaddr() + i * size_of::<T>())
    }
}

pub struct UPtr<T: Copy> {
    uaddr: usize,
    _marker: core::marker::PhantomData<T>,
}

impl<T: Copy> UPtr<T> {
    pub fn read(&self) -> SysResult<T> {
        debug_assert!(!self.is_null());
        copy_from_user::object(self.uaddr)
    }

    pub fn write(&self, value: T) -> SysResult<()> {
        debug_assert!(!self.is_null());
        copy_to_user::object(self.uaddr, value)
    }
}

impl<T: Copy> UserPointer<T> for UPtr<T> {
    fn uaddr(&self) -> usize {
        self.uaddr
    }

    fn from_uaddr(uaddr: usize) -> Self {
        UPtr {
            uaddr,
            _marker: core::marker::PhantomData,
        }
    }
}

impl_from_usize!(UPtr<T>);

impl<T: Copy> Into<usize> for UPtr<T> {
    fn into(self) -> usize {
        self.uaddr
    }
}

pub struct UArray<T: Copy> {
    uaddr: usize,
    _marker: core::marker::PhantomData<T>,
}

impl<T: Copy> UArray<T> {
    pub fn read(&self, offset: usize, buf: &mut [T]) -> SysResult<()> {
        debug_assert!(!self.is_null());
        copy_from_user::slice(self.uaddr + size_of::<T>() * offset, buf)
    }

    pub fn write(&self, offset: usize, buf: &[T]) -> SysResult<()> {
        copy_to_user::slice(self.uaddr + size_of::<T>() * offset, buf)
    }
}

impl<T: Copy> UserPointer<T> for UArray<T> {
    fn uaddr(&self) -> usize {
        self.uaddr
    }

    fn from_uaddr(uaddr: usize) -> Self {
        UArray {
            uaddr,
            _marker: core::marker::PhantomData,
        }
    }
}

impl_from_usize!(UArray<T>);

pub type UBuffer = UArray<u8>;

pub struct UString {
    uaddr: usize,
}

impl UString {
    pub fn read(&self) -> SysResult<String> {
        debug_assert!(!self.is_null());
        copy_from_user::string(self.uaddr)
    }

    pub fn write(&self, s: &str, max_size: usize) -> SysResult<usize> {
        debug_assert!(!self.is_null());
        copy_to_user::string(self.uaddr, s, max_size)
    }

    pub fn is_null(&self) -> bool {
        self.uaddr == 0
    }

    pub fn should_not_null(&self) -> SysResult<()> {
        if self.is_null() {
            Err(Errno::EINVAL)
        } else {
            Ok(())
        }
    }
}

impl From<usize> for UString {
    fn from(uaddr: usize) -> Self {
        UString { uaddr }
    }
}
