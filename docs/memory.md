# 内存管理

## 页面分配和页框

KernelX 使用基于伙伴系统（Buddy System）的页框分配器来管理物理内存。伙伴系统将物理内存划分为多个大小为 2 的幂次方的块（页框），并通过合并和拆分这些块来满足内存分配请求。

```rust
// src/kernel/mm/page.rs
/// 分配一个页面，返回内核虚拟地址
pub fn alloc() -> usize;
/// 分配一个清零的页面，返回内核虚拟地址
pub fn alloc_zero() -> usize;
/// 分配多个连续页面，返回内核虚拟地址，返回的地址是对齐的
pub fn alloc_contiguous(pages: usize) -> usize;
/// 释放一个页面，传入内核虚拟地址
pub fn free(page: usize);
/// 释放多个连续页面，传入内核虚拟地址和页面数
pub fn free_contiguous(addr: usize, pages: usize)

/// 复制页面内容，从 src 地址复制到 dst 地址
pub fn copy(src: usize, dst: usize)
/// 将页面内容清零，传入内核虚拟地址
pub fn zero(kpage: usize)
```

KernelX 还提供了更上层的 `PhysPageFrame` 结构体，用于管理物理页面的生命周期。`PhysPageFrame` 在创建时分配一个物理页面，并在销毁时自动释放该页面。

```rust
// src/kernel/mm/page.rs
impl PhysPageFrame {
    /// 新建或者分配一个物理页面
    pub fn new(page: usize) -> Self;
    pub fn alloc() -> Self;
    pub fn alloc_zeroed() -> Self;

    /// 复制页面内容，返回一个新的物理页面
    pub fn copy(&self) -> PhysPageFrame;
    /// 和 `slice` 交互
    pub fn copy_from_slice(&self, offset: usize, src: &[u8]);
    pub fn copy_to_slice(&self, offset: usize, dst: &mut [u8]);
    pub fn slice(&self) -> &mut [u8];

    /// 获取页面原始数据
    pub fn get_page(&self) -> usize;
    pub fn ptr(&self) -> *mut u8;
}
```

同时使用一个页面的 `PhysPageFrame` 和裸露的页面指针时，必须小心页面的生命周期，尤其是在竞态条件下，Rust 编译器无法保证页面指针在超过了保护锁的生命周期之后仍旧有效！它很可能会被其他任务释放掉，从而导致悬垂指针的问题。

## 虚拟内存管理和页表

在体系结构层，
