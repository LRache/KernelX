use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::ops::{Deref, DerefMut};
use core::ptr;

pub struct InitedCell<T> {
    value: UnsafeCell<MaybeUninit<T>>,
    is_inited: UnsafeCell<bool>,
}

impl<T> InitedCell<T> {
    /// 创建一个新的、未初始化的 `InitedCell`。
    pub const fn new() -> Self {
        Self {
            value: UnsafeCell::new(MaybeUninit::uninit()),
            is_inited: UnsafeCell::new(false),
        }
    }

    /// 初始化 `InitedCell`。
    ///
    /// # Panics
    /// 在 Debug 模式下，如果 `InitedCell` 已经被初始化，调用此方法会 panic。
    /// 在 Release 模式下，重复调用此方法会造成内存泄漏（旧值不会被 drop）。
    pub fn init(&self, value: T) {
        // 在 debug 模式下检查是否已经初始化
        debug_assert!(!self.is_inited(), "InitedCell has already been initialized.");

        unsafe {
            // 写入值并标记为已初始化
            *self.value.get() = MaybeUninit::new(value);
            *self.is_inited.get() = true;
        }
    }

    /// 检查 `InitedCell` 是否已经被初始化。
    #[inline]
    pub fn is_inited(&self) -> bool {
        unsafe { *self.is_inited.get() }
    }

    /// 获取内部值的不可变引用。
    ///
    /// # Panics
    /// 在 Debug 模式下，如果 `InitedCell` 未被初始化，调用此方法会 panic。
    /// 在 Release 模式下，如果未初始化就调用，会产生未定义行为。
    #[inline]
    fn get(&self) -> &T {
        // 在 debug 模式下，断言必须已经初始化
        debug_assert!(self.is_inited(), "Cannot access uninitialized InitedCell.");
        unsafe {
            // 因为我们已经（在debug模式下）检查过，或者用户保证了它已被初始化，
            // 所以可以安全地假设 MaybeUninit 包含一个有效的值。
            (*self.value.get()).assume_init_ref()
        }
    }
    
    /// 获取内部值的可变引用。
    ///
    /// # Panics
    /// 在 Debug 模式下，如果 `InitedCell` 未被初始化，调用此方法会 panic。
    /// 在 Release 模式下，如果未初始化就调用，会产生未定义行为。
    #[inline]
    fn get_mut(&self) -> &mut T {
        // 在 debug 模式下，断言必须已经初始化
        debug_assert!(self.is_inited(), "Cannot access uninitialized InitedCell.");
        unsafe {
            (*self.value.get()).assume_init_mut()
        }
    }
}

impl<T> Deref for InitedCell<T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.get()
    }
}

impl<T> DerefMut for InitedCell<T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.get_mut()
    }
}

impl<T> Drop for InitedCell<T> {
    fn drop(&mut self) {
        if self.is_inited() {
            unsafe {
                ptr::drop_in_place((*self.value.get()).as_mut_ptr());
            }
        }
    }
}

unsafe impl<T> Sync for InitedCell<T> {}
