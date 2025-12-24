# 文件系统

## 文件系统抽象

![文件系统抽象](./static/filesystem-abstract.svg)

KernelX 对所有的文件提供了四层抽象：

1. `FileOps` 

```rust
// src/fs/file/file.rs
pub struct FileFlags {
    pub readable: bool,
    pub writable: bool,
    pub blocked: bool
}

// src/fs/file/fileop.rs
pub trait FileOps: DowncastSync {
    fn read(&self, buf: &mut [u8]) -> SysResult<usize>;
    fn pread(&self, buf: &mut [u8], offset: usize) -> SysResult<usize>;
    fn write(&self, buf: &[u8]) -> SysResult<usize>;
    fn pwrite(&self, buf: &[u8], offset: usize) -> SysResult<usize>;

    fn readable(&self) -> bool;
    fn writable(&self) -> bool;
    fn set_flags(&self, flags: FileFlags);
    
    fn seek(&self, offset: isize, whence: SeekWhence) -> SysResult<usize>;
    fn ioctl(&self, _request: usize, _arg: usize, _addrspace: &AddrSpace);
    fn fstat(&self) -> SysResult<FileStat>;
    fn fsync(&self) -> SysResult<()>;
    
    fn get_inode(&self) -> Option<&Arc<dyn InodeOps>>;
    fn get_dentry(&self) -> Option<&Arc<Dentry>>;

    fn wait_event(&self, _waker: usize, _event: PollEventSet) -> SysResult<Option<FileEvent>>;
    fn wait_event_cancel(&self);
}
```

`FileFlags` 结构体用于记录文件的打开标志，例如是否可读、是否可写、是否阻塞等。

`FileOps` 定义了文件操作的接口，包含读写、文件状态获取、文件定位、`ioctl` 操作等方法。每个打开的文件都会对应一个 `FileOps` 实例，用于处理对该文件的各种操作。

`FileOps` 还记录了文件的打开标志（`FileFlags`），用于用户读写的时候判断操作是否合法，以及文件对应的 `Dentry` 和 `Inode` 对象，方便在文件操作中获取文件的元信息，也用于文件系统 `at` 类的系统调用。

`FileOps` 的实例包括了：

- 普通文件的 `File` 实现，用于处理常规文件的读写操作，底层是一个 `dyn InodeOps`，`File` 记录了当前文件的读写偏移量等状态信息，并通过 `InodeOps` 的 `readat` 和 `writeat` 等方法来进行实际的数据读写，同时将 `ioctl`、`fstat` 等操作转发到底层的 `InodeOps`。

- 字符设备文件的 `CharFile` 实现，用于处理字符设备的读写操作，底层是一个 `dyn CharDriverOps`，文件读写能够直接转发到底层的字符设备驱动，减少中间层，同时 `ioctl` 操作也能够直接调用字符设备驱动的 `ioctl` 方法，`seek` 等操作则返回错误。

- 管道文件的 `Pipe` 实现，用于处理管道的读写操作，底层是一个 `PipeInner` 对象，文件读写操作会转发到底层的 `Pipe` 对象，`ioctl`、`fstat` 等操作也会转发到底层的 `PipeInner` 对象。

`wait_event` 和 `wait_event_cancel` 方法用于实现文件的异步事件通知功能，支持 `poll`、`select` 和 `epoll` 等异步 I/O 模型。`wait_event` 方法会检测当前是否能够立即返回，如果可以则立刻返回 `Some(FileEvent)`，否则将当前任务添加到等待队列中，并返回 `Ok(None)`，等待文件状态变化时唤醒任务。`wait_event_cancel` 方法用于取消等待，将任务从等待队列中移除。

2. `InodeOps`

```rust
// src/fs/inde/inode.rs
pub trait InodeOps: DowncastSync {
    fn create(&self, _name: &str, _mode: Mode) -> SysResult<Arc<dyn InodeOps>>;

    fn link(&self, name: &str, target: &Arc<dyn InodeOps>) -> SysResult<()>;
    fn unlink(&self, _name: &str) -> SysResult<()>;
    fn symlink(&self, target: &str) -> SysResult<()>;

    fn readat(&self, _buf: &mut [u8], _offset: usize) -> SysResult<usize>;
    fn writeat(&self, _buf: &[u8], _offset: usize) -> SysResult<usize>;
    fn get_dent(&self, _index: usize) -> SysResult<Option<(DirResult, usize)>>;

    fn lookup(&self, _name: &str) -> SysResult<u32>;
    fn rename(&self, _old_name: &str, _new_parent: &Arc<dyn InodeOps>, _new_name: &str) -> SysResult<()>;
    fn readlink(&self, buf: &mut [u8]) -> SysResult<Option<usize>>;

    fn size(&self) -> SysResult<u64>;
    fn mode(&self) -> SysResult<Mode>;
    fn chmod(&self, _mode: Mode) -> SysResult<()>;
      
    fn owner(&self) -> SysResult<(Uid, Uid)>;
    fn chown(&self, uid: Option<Uid>, gid: Option<Uid>);

    fn inode_type(&self) -> SysResult<FileType>;

    fn sync(&self) -> SysResult<()>;

    fn fstat(&self) -> SysResult<FileStat>;

    fn truncate(&self, _new_size: u64) -> SysResult<()>;

    fn update_atime(&self, time: &Duration) -> SysResult<()>;
    fn update_mtime(&self, time: &Duration) -> SysResult<()>;
    fn update_ctime(&self, time: &Duration) -> SysResult<()>;

    fn wrap_file(self: Arc<Self>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps>;
}

```

`InodeOps` 定义了文件节点操作的接口，包含创建文件、链接文件、读写数据、目录项获取、文件查找、重命名、读取符号链接、获取和修改文件属性等方法。每个具体的文件系统实现都需要提供这些方法的具体实现。

`wrap_file` 方法用于将 `InodeOps` 包装成一个 `FileOps` 实例，进行动态类型派发，增加了代码实现的灵活性。例如 `/dev/null` 虽然是一个字符设备文件，但却可以进行普通文件的随机读写操作，因此它的 `InodeOps` 实现会将自己包装成一个普通文件的 `File` 实现，而非 `CharFile` 实现。

`owner`、 `chown`、`mode`、`chmod` 等方法用于实现文件权限管理功能，在打开文件的时候，内核会进行权限鉴定，确保用户对文件的访问权限合法。

3. `SuperBlockOps`

```rust
// src/fs/filesystem.rs
pub trait SuperBlockOps: Send + Sync {
    /// 获取根节点的 inode 号
    fn get_root_ino(&self) -> u32;

    /// 通过 inode 号获取 inode 对象
    fn get_inode(&self, ino: u32) -> SysResult<Arc<dyn InodeOps>>;

    /// 创建临时的文件节点，用于mkstemp等操作
    fn create_temp(&self, _mode: Mode) -> SysResult<Arc<dyn InodeOps>>;

    /// 卸载文件系统时，清理资源
    fn unmount(&self) -> SysResult<()>;

    /// 获取文件系统信息，用于 statfs 系统调用
    fn statfs(&self) -> SysResult<Statfs>;

    /// 同步更改到存储设备
    fn sync(&self) -> SysResult<()>;
}

`SuperBlockOps` 定义了超级块的操作接口，包含获取根节点、通过 inode 号获取 inode 对象、创建临时文件节点、卸载文件系统、获取文件系统信息和同步更改等方法。每个具体的文件系统实现都需要提供这些方法的具体实现。

`SuperBlockOps` 返回的 `Arc<dyn InodeOps>` 可以自己持有一份所有权来做缓存，`vfs`层面的 `inode` 缓存将不依赖于 `Arc` 的引用计数。

`get_root_ino` 方法用于获取文件系统的根节点的 inode 号，用于对整个文件系统的树形访问。

4. `FileSystemOps`

```rust
pub trait FileSystemOps: Send + Sync {
    fn create(&self, fsno: u32, driver: Option<Arc<dyn BlockDriverOps>>) -> SysResult<Arc<dyn SuperBlockOps>>;
}
```

`FileSystemOps` 定义了文件系统的创建接口，用来接收参数，创建具体的文件系统的超级块。在启动的时候，`VFS` 会注册所有支持的文件系统类型，将文件系统名称和具体的 `impl FileSystemOps` 关联起来。当用户通过 `mount` 系统调用挂载文件系统时，`VFS` 会根据传入的文件系统名称找到对应的 `FileSystemOps`，调用其 `create` 方法创建具体的超级块。

## VFS 层

VFS 层用于管理所有的文件系统实例，提供统一的文件系统接口给上层使用。VFS 层负责文件系统的挂载、卸载、路径解析、文件操作等功能。

```rust
// src/fs/vfs/vfs.rs
pub struct VirtualFileSystem {
    /// Inode 缓存
    pub(super) cache: inode::Cache,
    /// 已挂载的文件系统列表
    pub(super) mountpoint: Mutex<Vec<Arc<Dentry>>>,
    /// 超级块表
    pub superblock_table: Mutex<SuperBlockTable>,
    /// 已注册的文件系统类型
    pub(super) fstype_map: BTreeMap<&'static str, &'static dyn FileSystemOps>,
    /// 根目录 Dentry，路径解析的起点
    pub(super) root: InitedCell<Arc<Dentry>>,
}
```

### Dentry 设计

```rust
// src/fs/vfs/dentry.rs
pub struct Dentry {
    /// inode索引，包括了文件系统超级块号和inode号
    inode_index: Index,
    /// 文件名
    name: String,
    /// 父目录 Dentry
    parent: SpinLock<Option<Arc<Dentry>>>,
    /// 子目录和文件列表缓存
    children: SpinLock<BTreeMap<String, Weak<Dentry>>>,
    /// 关联的 inode 对象
    inode: SpinLock<Weak<dyn InodeOps>>,
    /// 挂载的文件系统根目录 Dentry
    mount_to: SpinLock<Option<Arc<Dentry>>>,
}
```

`Dentry` 代表文件系统中的目录项，用于内核的路径解析和目录结构管理。其中的 `children` 字段缓存了子文件，用于加速文件查找。同时，`Dentry` 还持有一个弱引用的 `InodeOps` 对象，用于获取文件的元信息。在 `inode` 缓存失效时，`Dentry` 会通过 `vfs` 层重新加载对应的 `InodeOps` 对象，这样可以防止系统中存在大量未使用的 `InodeOps` 对象占用内存。

`Dentry` 将 `lookup`、`create`、`link`、`unlink` 等方法封装在自身内部，方便上层调用，并将请求转发到底层的 `InodeOps` 对象，同时更新自身的 `children` 缓存。

### VFS 初始化

在内核启动的时候，启动代码会调用 `vfs::init` 方法初始化 VFS 层，最终调用 `register_filesystem` 方法注册内核支持的文件系统类型，例如 `ext4`、`devfs`、`tmpfs` 等，然后挂载一个空的根文件系统，初始化根目录 `root: InitedCell<Arc<Dentry>>`。 在文件系统初始化，挂载根文件系统的时候，真正的根文件系统会挂载到根目录 `root` 上。

```rust
// src/fs/vfs/init.rs
#[unsafe(link_section = ".text.init")]
pub fn init();
```

### 文件系统挂载

文件系统的挂载通过 `vfs::mount` 方法实现。该方法接收挂载点路径、文件系统类型、块设备驱动等参数，首先解析挂载点路径，找到对应的 `Dentry`，然后根据文件系统类型找到对应的 `FileSystemOps`，调用其 `create` 方法创建具体的超级块，最后将新的文件系统挂载到指定的挂载点 `Dentry` 上，同时，将挂载起点的 `Arc<Dentry>` 复制一份保存到 `mountpoint` 列表中，保证挂载点能够一直存在于内存中，由于 `Dentry` 持有了父目录的一份所有权，因此实际上根目录到该挂载点的整条目录查找链条都会被保留在内存中。

挂载的文件系统的超级块会被顺序分配一个唯一的文件系统号 `sno`，用于区分不同的文件系统实例。
VFS 采用 `InodeIndex {sno, ino}` 结构体来唯一标识一个文件系统中的文件节点。

挂载相关函数：

```rust
// src/fs/vfs/fsop.rs
impl VirtualFileSystem {
    fn mount(&self, path: &str, fstype_name: &str, device: Option<Arc<dyn BlockDriverOps>>) -> SysResult<()>;
}
```

### 路径解析

路径解析主要由 `lookup_dentry` 函数实现。

```rust
// src/fs/vfs/vfs.rs
impl VirtualFileSystem {
    pub fn lookup_dentry(&self, dir: &Arc<Dentry>, path: &str) -> SysResult<Arc<Dentry>>;

    /// 根据超级块号和inode号加载Inode对象b
    pub fn load_inode(&self, sno: u32, ino: u32) -> SysResult<Arc<dyn InodeOps>>;
}

// src/fs/vfs/dentry.rs
impl Dentry {
    /// 根据名称查找子目录项，如果缓存中不存在则从底层文件系统加载
    pub fn lookup(self: &Arc<Self>, name: &str) -> SysResult<Arc<Dentry>>;
    
    /// 根据名称查找子目录项，不使用缓存，直接从底层文件系统加载
    pub fn lookup_nocached(self: &Arc<Self>, name: &str) -> SysResult<Arc<Dentry>>;
    pub fn walk_link(self: Arc<Self>) -> SysResult<Arc<Dentry>>;
    pub fn get_mount_to(self: Arc<Self>) -> Arc<Dentry>;
}
```

路径解析函数会返回最后的 `Dentry`，如果路径中包含符号链接，则会自动解析符号链接指向的路径。

路径解析的步骤：

1. 根据传入的路径字符串，判断起始点是根目录还是当前目录 `dir` 。

2. 将路径按 `/` 分割成多个部分，依次处理每个部分。

3. 对于每个部分，调用当前目录 `Dentry` 的 `lookup` 方法查找子目录项。如果子目录项不存在，则返回错误。 `Dentry` 会先在自身的 `children` 缓存中查找，如果缓存中不存在，则调用 VFS 的 `load_inode` 方法，通过底层的 `InodeOps` 的 `lookup` 方法，获得 `name` 对应的 `inode` 编号，通过维护的超级块表，找到对应的超级块，调用超级块的 `get_inode` 方法，获得 `InodeOps` 实例，加载 Inode 。

4. 对找到的子目录项 `Dentry` 调用 `walk_link` 和 `get_mount_to` 方法，处理符号链接和挂载点的跳转。

5. 重复步骤 3 和 4，直到处理完所有路径部分，最终返回解析得到的 `Dentry`。

路径解析还有一些衍生函数，包括：

```rust
// src/fs/vfs/vfs.rs
impl VirtualFileSystem {
    pub fn lookup_dentry_nofollow(&self, dir: &Arc<Dentry>, path: &str) -> SysResult<Arc<Dentry>>;
    pub fn lookup_parent_dentry<'a>(&self, dir: &Arc<Dentry>, path: &'a str) -> SysResult<(Arc<Dentry>, &'a str)>;
}
```

## 特殊文件系统实现

### memtreefs tmpfs 和 devtmpfs

### procfs

## 外部接口

VFS 层提供了一些外部接口

```rust
// src/fs/vfs/fileop.rs
/// 加载指定路径的 Dentry 对象
pub fn load_dentry(path: &str) -> SysResult<Arc<Dentry>>
/// 加载指定路径的父目录的 Dentry 对象和最后一个路径的名称，用于创建新文件等操作
pub fn load_parent_dentry<'a>(path: &'a str) -> SysResult<(Arc<Dentry>, &'a str)>;
/// 打开指定路径的文件，返回对应的 FileOps 对象
pub fn open_file(path: &str, flags: FileFlags, perm: &Perm) -> SysResult<Arc<dyn FileOps>>;
/// 相对于指定目录加载 Dentry 对象
pub fn load_dentry_at(dir: &Arc<Dentry>, path: &str) -> SysResult<Arc<Dentry>>;
/// 相对于指定目录加载 Dentry 对象，不解析最后一个路径部分的符号链接
pub fn load_dentry_at_nofollow(dir: &Arc<Dentry>, path: &str) -> SysResult<Arc<Dentry>>;
/// 相对于指定目录加载父目录的 Dentry 对象并解析最后一个路径的名称
pub fn load_parent_dentry_at<'a>(dir: &Arc<Dentry>, path: &'a str) -> SysResult<Option<(Arc<Dentry>, &'a str)>>;
/// 创建指定路径的普通文件，返回对应的 FileOps 对象
pub fn create_file(dir: &Arc<Dentry>, name: &str, flags: FileFlags, mode: Mode) -> SysResult<Arc<dyn FileOps>>;
/// 创建指定路径的临时文件，返回对应的 FileOps 对象
pub fn create_temp(dentry: &Arc<Dentry>, flags: FileFlags, mode: Mode) -> SysResult<Arc<dyn FileOps>>;
```

这些接口可以由上层直接调用，用于加载文件、创建文件等操作。
