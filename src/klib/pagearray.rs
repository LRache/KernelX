use core::iter::FusedIterator;
use core::ops::{Index, IndexMut};

use crate::arch;
use crate::kernel::mm::page;

pub struct PageArray<T: Copy> {
    ptr: *mut T,
    length: usize,
    _marker: core::marker::PhantomData<T>
}

impl<T: Copy> PageArray<T> {
    pub fn new(length: usize, default: T) -> Self {
        debug_assert!(core::mem::size_of::<T>() > 0, "PageArray does not support zero-sized types");
        
        let size = length * core::mem::size_of::<T>();
        let ptr = page::alloc_contiguous(arch::page_count(size)) as *mut T;

        for i in 0..length {
            unsafe {
                ptr.add(i).write(default);
            }
        }

        Self { 
            ptr,
            length, 
            _marker: core::marker::PhantomData 
        }
    }

    pub fn get(&self, index: usize) -> T {
        debug_assert!(index < self.length);
        unsafe {
            self.ptr.add(index).read()
        }
    }

    pub fn set(&mut self, index: usize, value: T) {
        debug_assert!(index < self.length);
        unsafe {
            self.ptr.add(index).write(value);
        }
    }

    pub fn iter(&self) -> PageArrayIter<'_, T> {
        PageArrayIter {
            current: self.ptr,
            remaining: self.length,
            _marker: core::marker::PhantomData,
        }
    }
}

pub struct PageArrayIter<'a, T: Copy> {
    current: *const T,
    remaining: usize,
    _marker: core::marker::PhantomData<&'a T>,
}


impl<'a, T: Copy> Iterator for PageArrayIter<'a, T> {
    type Item = &'a T;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }

        unsafe {
            let value = &*self.current;
            self.current = self.current.add(1);
            self.remaining -= 1;
            Some(value)
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl<'a, T: Copy> ExactSizeIterator for PageArrayIter<'a, T> {
    fn len(&self) -> usize {
        self.remaining
    }
}

impl<'a, T: Copy> FusedIterator for PageArrayIter<'a, T> {}

impl<'a, T: Copy> IntoIterator for &'a PageArray<T> {
    type Item = &'a T;
    type IntoIter = PageArrayIter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<T: Copy> Index<usize> for PageArray<T> {
    type Output = T;

    fn index(&self, index: usize) -> &Self::Output {
        debug_assert!(index < self.length);
        unsafe { &*self.ptr.add(index) }
    }
}

impl<T: Copy> IndexMut<usize> for PageArray<T> {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        debug_assert!(index < self.length);
        unsafe { &mut *self.ptr.add(index) }
    }
}

impl<T: Copy> Drop for PageArray<T> {
    fn drop(&mut self) {
        let size = (self.length + core::mem::size_of::<T>() - 1) / core::mem::size_of::<T>();
        let page_count = arch::page_count(size);
        page::free_contiguous(self.ptr as usize, page_count);
    }
}
