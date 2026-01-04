use alloc::string::String;
use core::fmt::Debug;
use core::mem::size_of;

use crate::kernel::mm::ubuf::UAddrSpaceBuffer;
use crate::kernel::scheduler::current::{copy_from_user, copy_to_user};
use crate::kernel::scheduler::current;
use crate::kernel::errno::{SysResult, Errno};

/// Macro to implement From<usize> for user pointer types
macro_rules! impl_from_usize {
    ($type_name:ident<T>) => {
        impl<T: UserStruct> From<usize> for $type_name<T> {
            fn from(uaddr: usize) -> Self {
                $type_name {
                    uaddr,
                    _marker: core::marker::PhantomData,
                }
            }
        }
    };
}

pub trait UserStruct: Sized + Copy {}

impl UserStruct for u8 {}
impl UserStruct for usize {}
impl UserStruct for () {}

pub trait UserPointer<T: UserStruct> {
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

    fn kaddr(&self) -> SysResult<usize> {
        debug_assert!(!self.is_null());
        current::addrspace().translate_write(self.uaddr())
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct UPtr<T: UserStruct> {
    uaddr: usize,
    _marker: core::marker::PhantomData<T>,
}

impl<T: UserStruct> UPtr<T> {
    pub fn read(&self) -> SysResult<T> {
        debug_assert!(!self.is_null());
        copy_from_user::object(self.uaddr)
    }

    pub fn write(&self, value: T) -> SysResult<()> {
        debug_assert!(!self.is_null());
        copy_to_user::object(self.uaddr, value)
    }

    pub fn add(&self, offset: usize) -> Self {
        Self::from_uaddr(self.uaddr + offset * size_of::<T>())
    }

    pub fn read_optional(&self) -> SysResult<Option<T>> {
        if self.is_null() {
            Ok(None)
        } else {
            Ok(Some(self.read()?))
        }
    }
}

impl<T: UserStruct> UserPointer<T> for UPtr<T> {
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

impl <T: UserStruct> Debug for UPtr<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "UPtr({:#x})", self.uaddr)
    }
}

impl_from_usize!(UPtr<T>);

impl<T: UserStruct> Into<usize> for UPtr<T> {
    fn into(self) -> usize {
        self.uaddr
    }
}

pub struct UArray<T: UserStruct> {
    uaddr: usize,
    _marker: core::marker::PhantomData<T>,
}

impl<T: UserStruct> UArray<T> {
    pub fn read(&self, offset: usize, buf: &mut [T]) -> SysResult<()> {
        debug_assert!(!self.is_null());
        copy_from_user::slice(self.uaddr + size_of::<T>() * offset, buf)
    }

    pub fn write(&self, offset: usize, buf: &[T]) -> SysResult<()> {
        copy_to_user::slice(self.uaddr + size_of::<T>() * offset, buf)
    }

    pub fn index(&self, i: usize) -> UPtr<T> {
        UPtr::from_uaddr(self.uaddr + i * size_of::<T>())
    }
}

impl<T: UserStruct> UserPointer<T> for UArray<T> {
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

impl UBuffer {
    pub fn to_uaddrspace_buffer(&self, length: usize) -> UAddrSpaceBuffer<'_> {
        UAddrSpaceBuffer::new(self.uaddr, length, current::addrspace())
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
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

impl UserStruct for UString {}

impl From<usize> for UString {
    fn from(uaddr: usize) -> Self {
        UString { uaddr }
    }
}

impl<T: UserStruct> From<UPtr<T>> for UString {
    fn from(value: UPtr<T>) -> Self {
        Self { uaddr: value.uaddr() }
    }
}
