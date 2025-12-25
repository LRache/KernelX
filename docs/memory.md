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

KernelX 定义了三种内存地址类型：

- `paddr`: CPU 访问的真实物理地址。

- `kaddr`: 内核的虚拟地址，在支持的体系结构上，不同SoC上启动的内核会将自己映射到一个高地址上。

- `uaddr`: 用户虚拟地址。

### 页表

在体系结构层，定义了页表接口：

```rust
pub trait PageTableTrait {
    /// 映射 uaddr -> kaddr，用于映射普通用户页面
    fn mmap(&mut self, uaddr: usize, kaddr: usize, perm: MapPerm);
    /// 映射 kaddr -> paddr，用于映射 MMIO 端口等
    fn mmap_paddr(&mut self, kaddr: usize, paddr: usize, perm: MapPerm);
    /// 替换映射
    fn mmap_replace(&mut self, uaddr: usize, kaddr: usize, perm: MapPerm);
    /// 替换映射中的页面
    fn mmap_replace_kaddr(&mut self, uaddr: usize, kaddr: usize);
    /// 替换映射中的权限位
    fn mmap_replace_perm(&mut self, uaddr: usize, perm: MapPerm);
    /// 取消映射
    fn munmap(&mut self, uaddr: usize);
    /// 取消映射，返回原本映射是否存在
    fn munmap_with_check(&mut self, uaddr: usize, expected_kaddr: usize) -> bool;
    /// 获取并清空一页 access 位和 dirty 位
    fn take_access_dirty_bit(&mut self, uaddr: usize) -> Option<(bool, bool)>;
}
```

注意 `mmap` 等函数的输入是 `kaddr`，在做实际映射的实现时，需要将 `kaddr` 转换为对应的 `paddr`。

`mmap_paddr` 函数用于映射内核空间的物理地址，比如设备 MMIO 端口等。

### 内核映射

#### 内核映射

内核在启动的时候，会映射自身所在的物理内存区域到内核虚拟地址空间中。映射的权限基于内核在链接时候定义的符号，符号应该由体系结构相关的内核启动代码定义。

#### 跳板代码和VDSO映射

内核映射了一段跳板代码，用于从用户模式陷入，但是还没有切换到内核页表的临界区域，在这段代码中，内核会保存用户程序的上下文，切换栈为内核栈，并且切换到内核页表，以便进行后续的内核处理。同样，从内核返回用户态时，也会使用这段跳板代码，以确保页表切换和栈切换的正确性。跳板代码的映射权限应该是用户态不可访问的，跳板代码映射由体系结构层实现。

VDSO 是用户可以访问的一段内核映射代码，VDSO 在用户地址空间新建的时候，和跳板代码一起映射到用户地址空间中。

#### 内核栈保护

为了防止内核栈溢出导致问题，KernelX 在每个内核栈的底部映射了一页不可访问的保护页。当内核栈溢出时，访问保护页会触发页面错误，从而防止栈溢出覆盖其他内存区域。在创建内核栈的时候，会自动映射内核栈的保护页，并在销毁的时候恢复。体系结构层的代码可以在处理内核陷入的时候，检查是否访问了保护页，从而检测内核栈溢出的问题。

## 用户映射管理

### 映射区域

KernelX 使用 `MapArea` 结构体来表示用户地址空间中的一个映射区域：

```rust
// src/kernel/mm/maparea/area.rs
pub trait Area {
    /// 翻译用户地址到内核地址
    fn translate_read (&mut self, uaddr: usize, addrspace: &AddrSpace) -> Option<usize>;
    fn translate_write(&mut self, uaddr: usize, addrspace: &AddrSpace) -> Option<usize>;
    /// 映射起始地址
    fn ubase(&self) -> usize;
    fn set_ubase(&mut self, _ubase: usize);
    /// 映射权限
    fn perm(&self) -> MapPerm;
    fn set_perm(&mut self, _perm: MapPerm, _pagetable: &RwLock<PageTable>);
    /// 复制映射区域，用于 fork
    fn fork(&mut self, self_pagetable: &RwLock<PageTable>, fork_pagetable: &RwLock<PageTable>) -> Box<dyn Area>;
    /// 处理内存访问错误，尝试修复，修复成功则返回 true
    fn try_to_fix_memory_fault(
        &mut self, 
        uaddr: usize, 
        access_type: MemAccessType, 
        addrspace: &Arc<AddrSpace>
    ) -> bool;

    fn page_count(&self) -> usize;
    fn size(&self) -> usize;
    
    /// 将映射区域一分为二，返回两个新的映射区域
    fn split(self: Box<Self>, _uaddr: usize) -> (Box<dyn Area>, Box<dyn Area>);
    
    fn unmap(&mut self, _pagetable: &RwLock<PageTable>);

    fn type_name(&self) -> &'static str;
}
```

`MapArea` 提供了翻译用户地址到内核地址的接口 `translate_read` 和 `translate_write`，在内核尝试访问用户地址空间的时候，例如读取用户缓冲区或者写入用户缓冲区的时候，需要使用这些接口将用户地址转换为内核地址，同时这些函数会处理懒分配或者写时复制（CoW）等机制，确保内核访问的是正确的内存页面。

`try_to_fix_memory_fault` 方法用于处理用户内存访问错误。当出现页面错误的时候，可能是访问了 CoW 页面，或者是访问了尚未分配的页面。该方法尝试修复这些错误，比如为尚未分配的页面分配新的页框，或者为 CoW 页面创建新的私有副本。如果修复成功，返回 `true`，否则返回 `false`。

`split` 方法用于将一个映射区域拆分为两个新的映射区域，并消耗掉原有的区域对象。拆分操作通常在修改映射权限或者取消映射的时候使用，因为这些操作可能只影响映射区域的一部分。

### 映射区域管理

`MapManager` 结构体用于管理多个 `MapArea`，并提供接口来添加、删除和查找映射区域：

```rust
pub struct Manager {
    /// 映射区域集合，按起始地址排序
    areas: BTreeMap<usize, Box<dyn Area>>,
    /// 用户栈的起始地址
    userstack_ubase: usize,
    /// 用户 brk 的起始地址
    userbrk: UserBrk,
}

impl Manager {
    pub fn new() -> Self;
    /// 复制映射区域，用于 fork
    pub fn fork(&mut self, self_pagetable: &RwLock<PageTable>, new_pagetable: &RwLock<PageTable>) -> Self;
    /// 查找一块能够映射指定页面数的空闲区域的起始地址，用于未指定地址的 mmap 调用
    pub fn find_mmap_ubase(&self, page_count: usize) -> Option<usize>;
    /// 检查指定范围内是否有映射区域重叠，用于 mmap 调用前的检查
    pub fn is_map_range_overlapped(&self, start: usize, page_count: usize) -> bool;
    /// 添加一个映射区域
    pub fn map_area(&mut self, uaddr: usize, area: Box<dyn Area>);

    /// 辅助函数，查找和指定范围重叠的所有映射区域的起始地址
    fn find_overlapped_areas(&self, start: usize, end: usize) -> Vec<usize>;
  
    /// 在指定地址映射一个区域，替换掉重叠的区域
    pub fn map_area_fixed(&mut self, uaddr: usize, area: Box<dyn Area>, pagetable: &RwLock<PageTable>);
    /// 取消映射指定范围内的区域
    pub fn unmap_area(&mut self, uaddr: usize, page_count: usize, pagetable: &RwLock<PageTable>) -> SysResult<()>;
    /// 设置指定范围内的映射区域的权限，设置权限的区域可能会被拆分
    pub fn set_map_area_perm(&mut self, uaddr: usize, page_count: usize, perm: MapPerm, pagetable: &RwLock<PageTable>) -> SysResult<()>;
    /// 创建用户栈映射区域，用于新建进程初始化阶段
    pub fn create_user_stack(&mut self, argv: &[&str], envp: &[&str], auxv: &Auxv, addrspace: &AddrSpace) -> SysResult<usize>;

    /// 翻译用户地址到内核地址，用于读取
    pub fn translate_read(&mut self, uaddr: usize, addrspace: &AddrSpace) -> Option<usize>;
    /// 翻译用户地址到内核地址，用于写入
    pub fn translate_write(&mut self, uaddr: usize, addrspace: &AddrSpace) -> Option<usize>;

    /// 处理内存访问错误，尝试修复，修复成功则返回 true
    pub fn try_to_fix_memory_fault(&mut self, uaddr: usize, access_type: MemAccessType, addrspace: &Arc<AddrSpace>) -> bool;
    /// 增加用户 brk，返回新的 brk 地址
    pub fn increase_userbrk(&mut self, new_ubrk: usize) -> SysResult<usize>;
}
```

`MapManager` 中记录了区域起始地址-区域对象的映射关系，使用 `BTreeMap` 来存储，在查找指定用户地址映射的区域的时候，利用 `BTreeMap` 的有序性，只需要查找最后一个小于等于 `uaddr` 的键，能够高效地定位到对应的区域。

`translate_read` 、`translate_write`、 `try_to_fix_memory_fault` 等方法会先查找对应的区域，然后将调用转发到具体的 `MapArea` 对象上。`try_to_fix_memory_fault` 方法会提前检测访问权限是否符合要求，如果不符合要求，则直接返回 `false`，避免不必要的函数调用开销。`MapArea` 内部实现也无需处理权限检查的问题。

`map_area` 函数会将一个新的映射区域添加到 `BTreeMap` 中，`unmap_area` 则会在调用了 `MapArea` 的 `unmap` 方法之后，将对应的区域从 `BTreeMap` 中删除。`unmap` 需要实际传递页表对象，以便取消映射。

`fork` 方法用于地址空间的复制，在创建子进程的时候，会调用该方法来复制父进程的映射区域。`fork` 方法会遍历所有的映射区域，并调用每个区域的 `fork` 方法来创建新的区域对象，然后将这些新的区域添加到新的 `MapManager` 中。`fork` 方法需要传入新旧页表，以便处理 CoW 页面的映射权限问题。

`set_perm` 、`map_area_fixed`、`unmap` 等方法可能需要拆分已有的映射区域，因为修改权限或者取消映射的范围可能只覆盖了部分区域。 拆分的步骤如下：

1. 使用 `find_overlapped_areas` 辅助函数查找所有和指定范围重叠的区域起始地址。

2. 对每个重叠的区域，调用 `split` 方法将其拆分为多个区域：[`left`, `middle`, `right`]，其中 `middle` 是和指定范围重叠的部分，`left` 和 `right` 是不重叠的部分。

3. 对 `middle` 区域进行权限修改或者取消映射操作。

`create_user_stack` 方法用于创建用户栈映射区域，在新建进程初始化阶段调用。该方法会创建一个新的栈映射区域，将传入的 `argv`、`envp` 和 `auxv` 压入用户栈中，并将其添加到 `MapManager` 中。

`increase_userbrk` 方法用于增加用户堆的大小，它会在原本的 `userbrk` 基础上增加指定的大小，并创建新的匿名映射区域来覆盖新增的堆空间。

### 用户地址空间

用户地址空间主要由 `AddrSpace` 结构体管理：

```rust
// src/kernel/mm/addrspace.rs
pub struct AddrSpace {
    /// 管理用户地址空间的映射区域
    map_manager: Mutex<maparea::Manager>,
    /// 体系结构相关的页表
    pagetable: RwLock<PageTable>,
    /// 用户上下文使用的页框
    usercontext_frames: Mutex<Vec<PhysPageFrame>>,
}
```

`pagetable` 字段保存了体系结构相关的页表对象，用于实际的地址转换和内存映射操作。 `map_manager` 字段保存了 `MapManager` 对象，用于管理用户地址空间中的映射区域。

![AddrSpace 结构体示意图](./static/addrspace.svg)

```rust
// src/kernel/mm/addrspace.rs
impl AddrSpace {
    /// 新建一个地址空间
    pub fn new() -> Arc<Self>;
    
    /// 复制地址空间，用于 fork
    pub fn fork(self: &Arc<Self>) -> Arc<AddrSpace>;

    /// 分配一个用户上下文页，返回用户地址和内核地址
    pub fn alloc_usercontext_page(&self) -> (usize, *mut UserContext);
    /// 创建用户栈映射区域，用于新建进程初始化阶段
    pub fn create_user_stack(&self, argv: &[&str], envp: &[&str], auxv: &Auxv) -> SysResult<usize>;
    /// 映射到 MapManager 的接口
    pub fn map_area(&self, uaddr: usize, area: Box<dyn maparea::Area>) -> SysResult<()>;
    pub fn set_area_perm(&self, uaddr: usize, page_count: usize, perm: MapPerm) ->SysResult<()>;
    pub fn increase_userbrk(&self, ubrk: usize) -> SysResult<usize>;
    pub fn translate_write(self: &Arc<Self>, uaddr: usize) -> SysResult<usize>;

    /// 跨用户态和内核态的数据复制接口
    pub fn copy_to_user_buffer(&self, mut uaddr: usize, buffer: &[u8]) -> SysResult<()>;
    pub fn copy_to_user<T: Copy>(&self, uaddr: usize, value: T) -> SysResult<()>;
    pub fn copy_to_user_slice<T>(&self, uaddr: usize, slice: &[T]) -> SysResult<()>;
    pub fn copy_from_user<T: Copy>(&self, uaddr: usize) -> SysResult<T>;
    pub fn get_user_string(&self, mut uaddr: usize) -> SysResult<String>;
    pub fn copy_from_user_slice<T: Copy>(&self, uaddr: usize, slice: &mut [T]) -> SysResult<()>;

    /// 访问页表的接口
    pub fn with_pagetable<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&PageTable) -> R;

    pub fn pagetable(&self) -> &RwLock<PageTable>;

    pub fn with_map_manager_mut<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut maparea::Manager) -> R;

    /// 处理内存访问错误，尝试修复，修复成功则返回 true
    pub fn try_to_fix_memory_fault(self: &Arc<Self>, uaddr: usize, access_type: MemAccessType) -> bool;
}
```

`AddrSpace` 绑定了 `PageTable` 和 `MapManager`，将一些需要传入 `PageTable` 的 `MapManager` 方法进行了封装，比如 `map_area` 和 `set_area_perm` 等方法，上层在调用这些方法时不需要显式传入页表对象。

同时，`AddrSpace` 封装了一系列跨用户态和内核态的数据复制接口，比如 `copy_to_user` 和 `copy_from_user` 等方法，这些方法会先使用 `MapManager` 将用户地址转换为内核地址，然后进行数据复制操作。在跨用户态和内核态访问数据实现中，会自动检测跨页面边界的情况，只有在跨越页面的时候，才会触发再次地址转换，从而提高性能。这些接口的底层实际上都是 `copy_to_user_buffer` 和 `copy_from_user_buffer` 方法，其他函数会将传入的数据转换为 `buffer` 形式，然后调用这两个方法进行实际的数据复制。

KernelX 对 Memory Fault 的处理过程：

![memory fault 处理流程图](./static/memory_fault.svg)

### 懒分配和写时复制

KernelX 支持懒分配（Lazy Allocation）和写时复制（Copy-on-Write, CoW）机制，以提高内存使用效率。

在创建任何用户映射的时候，内核只是记录了映射区域的信息，包括映射的范围、权限等，但是并不会为这些映射区域分配实际的物理页面，也不会在页表上做实际的映射。只有当用户程序第一次访问这些映射区域时，才会触发页面错误，内核会在页面错误处理函数中为该页面分配一个新的物理页面，并更新页表中的映射关系。`MapArea` 在 `fork` 的时候，会将原有的映射区域复制一份，并将权限修改为只读，从而实现写时复制机制。当子进程或者父进程尝试写入该页面时，会触发页面错误，内核会为该页面分配一个新的物理页面，并将数据从原有页面复制到新页面中，然后更新页表中的映射关系，将权限修改为可写。

大多数的 `MapArea` 都采用了这样的页面状态：

```rust
// src/kernel/mm/maparea/area.rs
pub enum Frame {
    /// 页面尚未分配
    Unallocated,
    /// 页面已分配
    Allocated(Arc<PhysPageFrame>),
    /// 写时复制页面
    Cow(Arc<PhysPageFrame>),
}
```

`MapArea` 内部会持有一个 `Frame` 的列表，用来记录每个页面的状态。在处理页面错误的时候，会根据页面的状态来决定如何处理该错误，并更新页面状态：

- 如果是 `Unallocated` 状态，则分配一个新的页面，页面的内容初始化为 0 或者从文件中读取。

- 如果是 `Cow` 状态，检查自己是否是这个页面的唯一持有者：

  + 如果是唯一持有者，说明没有其他进程共享该页面，可以直接将页面状态修改为 `Allocated`，并将权限修改为可写。

  + 如果不是唯一持有者，说明有其他进程也在使用该页面，需要分配一个新的页面，将原页面的内容复制到新页面中，然后将新页面的状态设置为 `Allocated`，并更新页表中的映射关系。

- 如果是 `Allocated` 状态，说明页面已经是可写的，这种情况不应该发生，或者可能是页面被换出到外部存储器上。

在处理 `fork` 操作的时候，如果当前映射区域没有被标记为 SHARED， 就会将所有的 `Allocated` 页面状态修改为 `Cow`，并将权限修改为只读， `Unallocated` 页面保持不变。

## 页面换入换出

KernelX 支持页面换入换出机制，以便在物理内存不足时，将不常用的页面换出到外部存储器（如磁盘）上，从而释放物理内存供其他进程使用。

### 换入换出策略

KernelX 定义了内核也分配的高水位线和低水位线，当物理内存使用超过高水位线时，内核会触发页面换出操作，将一些不常用的页面换出到外部存储器上，尝试将物理内存使用降到低水位线以下为止。

```rust
// src/kernel/mm/page.rs
#[cfg(feature = "swap-memory")]
pub fn alloc_with_shrink_zero() -> usize;

impl PageFrame {
     #[cfg(feature = "swap-memory")]
    pub fn alloc_with_shrink_zeroed() -> Self;
}
```

只有调用了 `alloc_with_shrink_zero` 方法分配的页面，才会参与换出操作。在这些方法中，如果物理内存不足，会触发页面换出操作，释放一些页面，然后再尝试分配新的页面。

所有可被换出的页面需要实现 `SwappableFrame` ：

```rust
pub trait SwappableFrame {
    /// 换出页面，dirty 表示页面是否被修改过，返回是否成功换出
    fn swap_out(&self, dirty: bool) -> bool;
    /// 获取并清空页面的 access 位和 dirty 位，用于换出决策
    fn take_access_dirty_bit(&self) -> Option<(bool, bool)>;
}
```

`swap_out` 方法用于将页面换出到外部存储器上，`dirty` 参数表示页面是否被修改过，可以用于优化换出操作是否需要磁盘写入。该方法返回一个布尔值，表示是否成功换出页面。

KernelX 目前采用了最简单的换出 LRU 策略，根据页面的访问位（access bit）来决定页面是否被频繁访问，从而决定是否换出该页面。`take_access_dirty_bit` 方法会获取并清空页面的访问位和修改位，用于换出决策。如果页面的访问位为 `false`，说明该页面在最近一段时间内没有被访问过，可以考虑换出该页面。如果访问位为 `true`，说明该页面被频繁访问，不应该换出该页面。具体的逻辑在`Swapper` 中实现。

```rust
// src/kernel/mm/swappable/swapper.rs
struct SwapEntry {
    frame: Arc<dyn SwappableFrame>,
    dirty: bool,
}

struct Swapper {
    lru: SpinLock<LRUCache<usize, SwapEntry>>,
}

impl Swapper {
    /// 尝试换出页面，page_count 指定要换出的页面数，min_to_shrink 指定最少要换出的页面数
    fn shrink(&self, page_count: usize, min_to_shrink: usize)
}

pub fn push_lru(kpage: usize, frame: Arc<dyn SwappableFrame>)；
pub fn remove_lru(kpage: usize);
pub fn shrink(page_count: usize, min_to_shrink: usize);
```

`Swapper` 维护了一个 LRU 缓存，用于记录所有可换出的页面。所有的可换出页面在创建的时候，都应该使用 `push_lru` 把自己注册到 `Swapper` 中，当页面被释放的时候，也应该使用 `remove_lru` 从 `Swapper` 中注销自己。

shrink 方法的具体逻辑：

1. 首先遍历 LRU 缓存中最不经常访问的 `2 * page_count` 个页面，记录它们的 `dirty` 位，检查它们的 `access` 位：

   - 如果访问位为 `false`，说明该页面没有被频繁访问，可以考虑换出该页面。

   - 如果访问位为 `true`，说明该页面被频繁访问，不应该换出该页面，将其重新放回 LRU 。

2. 如果换出的页面数量小于 `min_to_shrink`，则继续遍历 LRU 缓存中剩余的页面，强制换出 LRU 缓存中最不经常访问的页面，即使它们近期被访问过，直到换出的页面数量达到 `min_to_shrink` 为止。

### kswapd 守护线程

除了内核主动触发的页面换出操作之外，KernelX 还启动了一个名为 `kswapd` 的内核守护线程，用于在后台监控物理内存使用情况，每 0.5s 检测一次物理内存使用率，如果发现物理内存使用超过高水位线，则触发页面换出操作，尝试将物理内存使用降到低水位线以下为止。

### nofile 页面的换入换出实现

nofile 页面是指不对应任何文件的匿名页面，比如用户堆栈、匿名映射等。

```rust
// src/kernel/mm/swappable/nofile/frame.rs
impl SwappableNoFileFrame {
    /// 创建一个新的 swappable nofile 页面
    pub fn allocated(uaddr: usize, frame: PhysPageFrame, addrspace: &AddrSpace) -> Self;
    pub fn alloc_zeroed(uaddr: usize, addrspace: &AddrSpace) -> (Self, usize);

    /// 复制页面内容，返回一个新的 swappable nofile 页面
    pub fn copy(&self, addrspace: &AddrSpace) -> (Self, usize);

    /// 获取页面的内核虚拟地址,如果页面已换出则返回 None
    pub fn get_page(&self) -> Option<usize>;
    /// 获取页面的内核虚拟地址，如果页面换出，则换入页面
    pub fn get_page_swap_in(&self) -> usize;
    /// 获取用户虚拟地址
    pub fn uaddr(&self) -> usize;
    /// 检查页面是否已换出
    pub fn is_swapped_out(&self) -> bool;
}
```

`Arc<SwappableNoFileFrame>` 是被 `MapArea` 持有所有权的页框对象。当 `MapArea` 尝试分配页面的时候，会获得一个 `Arc<SwappableNoFileFrame>` 对象作为页框对象。注意，和 CoW 机制不同，页面是否被换出的状态不由 `MapArea` 管理，`MapArea` 只需要持有 `Arc<SwappableNoFileFrame>` 对象即可，页面的换入换出状态由 `SwappableNoFileFrame` 内部管理。只有当页面被换出后，用户程序再次访问导致页面错误时，`MapArea` 才会根据情况，将页面换入到内存中，并重新完成映射，例如调用 `get_page_swap_in` 方法来加载页面并获取内核地址。

页面被换出后，需要通过逆映射找到所有映射了该页面的 `AddrSpace`，并取消这些映射关系。注意到，NoFile 页面是通过 `AddrSpace` 的 `fork` 来传播的，我们为每个 `AddrSpace` 维护了一个包含了 `AddrSpace` 弱引用的链表,也就是地址空间家族链。当 `AddrSpace` 通过 `fork` 创建新的地址空间时，会将新的地址空间添加到链表中。在新建 `SwappableNoFileFrame` 对象时，`SwappableNoFileFrame` 会保存一个指向了地址空间家族链的指针。这个家族链中的 `AddrSpace` ，就是所有的可能映射了该页面的地址空间。在换出页面时，`SwappableNoFileFrame` 会遍历这个家族链，找到所有映射了该页面的地址空间，并取消这些映射关系。

```rust
// src/kernel/mm/swappable/mod.rs
pub type AddrSpaceFamilyChain = Arc<SpinLock<LinkedList<Weak<AddrSpace>>>>;
```

`SwappableNoFileFrame` 内部有一个 `SwappableNoFileFrameInner` 指针，它实现了 `SwappableFrame` 接口，是换入换出实现的核心。

```rust
/// 表示一个已分配的页面及其脏位状态
pub(super) struct AllocatedFrame {
    pub(super) frame: PhysPageFrame,
    pub(super) dirty: bool,
}

pub(super) enum State {
    /// 页面已分配
    Allocated(AllocatedFrame),
    /// 页面已换出
    SwappedOut,
}

pub(super) struct FrameState {
    /// 页面状态
    pub(super) state: State,
    /// 页面在磁盘上的槽位
    pub(super) disk_slot: usize,
}

pub struct SwappableNoFileFrameInner {
    /// 页面状态
    pub(super) state: SpinLock<FrameState>,
    /// 所属的地址空间链
    pub(super) family_chain: AddrSpaceFamilyChain,
    /// 用户虚拟地址
    pub uaddr: usize,
}

pub struct SwappableNoFileFrame {
    inner: Arc<SwappableNoFileFrameInner>,
}
```

`SwappableNoFileFrameInner` 实现了 `SwappableFrame` 接口，是可以被换入换出的页面，由 `Swapper` 管理是否要换入换出，换入换出的具体逻辑则由一个全局的 `SwapperDisk` 实现。

`SwappableNoFileFrame` 中记录了页面所属的地址空间家族链 `family_chain`、页面对应的用户虚拟地址 `uaddr` 和页面的当前状态 `state`。 `state` 记录了页面当前是已分配状态还是已换出状态，以及页面在磁盘上的存储位置等信息。如果页面被换出，`state` 会被设置为 `SwappedOut`，`disk_block` 会记录页面在磁盘上的存储位置。如果页面是已分配状态，`state` 会保存一个 `AllocatedFrame` 结构体，包含了实际的物理页面和脏位信息，`disk_block` 为磁盘上的缓存位置。

具体的换入换出逻辑实现在 `SwapperDisk` 中：

```rust
// src/kernel/mm/swappable/nofile/swapper.rs
struct BitMap {
    allocated: BitVec,
    cached: BitVec, // `1` means this block has a cached page in memory `0` means this block only has a swapped out page on disk
}

struct SwapperDisk {
    bitmap: SpinLock<BitMap>,
    driver: Arc<dyn BlockDriverOps>,
    block_per_page: usize,
}

impl SwapperDisk {
    fn new(driver: Arc<dyn BlockDriverOps>) -> Self;

    pub fn read_page(&self, slot: usize, frame: &PhysPageFrame);
    pub fn write_page(&self, slot: usize, frame: &PhysPageFrame);

    pub fn alloc_slot(&self) -> Option<usize>;
    pub fn free_slot(&self, pos: usize);
}
```

`SwapperDisk` 使用一个位图来管理磁盘上的页面存储位置，在初始化的时候，会根据所使用的块设备大小，计算出可以存储的页面数量，并初始化位图。磁盘上存储一个页面的单位称为槽位 `slot`，每个槽位包含了若干个块（block）。

`alloc_slot` 方法用于分配一个新的槽位，用于存储换出的页面。该方法会在位图中查找一个未分配的槽位，并将其标记为已分配，返回槽位的索引号。如果没有可用的槽位，返回 `None`。 `free_slot` 方法用于释放一个已分配的槽位，将其标记为未分配，以便后续可以重新使用该槽位。


```rust
// src/kernel/mm/swappable/nofile/swap.rs
impl SwappableFrame for SwappableNoFileFrameInner {
    fn swap_out(&self, dirty: bool) -> bool;
    fn take_access_dirty_bit(&self) -> Option<(bool, bool)>;
}

impl SwappableNoFileFrameInner {
    pub fn get_page_swap_in(self: &Arc<Self>) -> usize;

    pub fn copy(&self, addrspace: &AddrSpace) -> (Arc<SwappableNoFileFrameInner>, usize);

    pub fn free(&self);
}

```

nofile 页面换出的具体逻辑如下：

1. 查询自身是否有已分配的槽位，如果没有，则尝试请求分配一个槽位。

2. 如果当前页面是脏页或者磁盘上不存在该页面的缓存（即自身没有已分配的槽位），则将页面内容写入到分配的槽位对应的磁盘位置。

3. 遍历地址空间家族链，找到所有映射了该页面的地址空间，并取消这些映射关系。

4. 更新页面状态为已换出状态，自动释放物理页面。

nofile 页面换入的具体逻辑如下：

1. 申请一个新的物理页面。

2. 从磁盘上读取页面内容到新分配的物理页面中。

3. 将自身重新放回到 LRU 缓存中，以便后续可能的换出操作。

4. 更新页面状态为已分配状态，保存新的物理页面。

换入的时候，内核并不会直接释放磁盘上的槽位，而是在页面被标记为脏页或者页面释放的时候，才会释放对应的槽位。这样可以避免写回干净的页面到磁盘中，减少磁盘写入操作。虽然这样可能会浪费一些磁盘空间，但是可以提高整体的性能表现。标记脏页的位置位于 `take_access_dirty_bit` 方法中，当页面被访问或者修改时，会更新页面的脏位信息。
