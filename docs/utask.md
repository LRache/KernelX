# 用户任务管理

## TCB 和 PCB

KernelX 使用 `TCB` 结构来表示用户任务（用户线程），使用 `PCB` 来表示用户进程，对应 Linux 内核的线程组。

```rust
// src/kernel/task/tcb.rs
pub struct TaskStateSet {
    /// 任务状态
    pub state: TaskState,
    /// 待处理的信号
    pub pending_signal: Option<PendingSignal>,
    /// 任务正在等待的信号集合
    pub signal_to_wait: SignalSet,
}

pub struct TCB {
    /// 线程 ID
    tid: Tid,
    /// 任务创建时间
    create_time: Duration,
    /// 所属线程组，即进程
    parent: Arc<PCB>,
    /// 退出时写回到用户空间的地址
    tid_address: Mutex<Option<usize>>,
    
    pub robust_list: SpinLock<Option<usize>>,
    
    /// 用户态上下文指针
    user_context_ptr: *mut UserContext,
    /// 用户态上下文的用户空间地址
    user_context_uaddr: usize,
    /// 内核态上下文和内核栈
    kernel_context: KernelContext,
    pub kernel_stack: KernelStack,

    /// 地址空间
    addrspace: Arc<AddrSpace>,
    /// 文件描述符表
    fdtable: Arc<SpinLock<FDTable>>,
    /// 信号掩码
    pub signal_mask: SpinLock<SignalSet>,
    /// 任务状态
    state: SpinLock<TaskStateSet>,
    /// 被唤醒原因
    pub wakeup_event: SpinLock<Option<Event>>,
    /// 等待自己的父进程（vfork）
    parent_waiting_vfork: SpinLock<Option<Arc<dyn Task>>>,
    /// 时间计数器，用于统计用户态和内核态时间
    pub time_counter: SpinLock<TimeCounter>,
}
```

`state` 中，待处理的信号集合和任务状态用一把锁保护，防止竞态条件。

`PCB` 结构如下：

```rust
// src/kernel/task/pcb.rs
pub struct PCB {
    /// 进程 ID，和主线程的线程 ID 相同
    pid: Tid,
    /// 父进程
    pub parent: SpinLock<Option<Arc<PCB>>>,
    /// 进程状态
    state: SpinLock<State>,
    /// 进程的可执行文件路径
    exec_path: SpinLock<String>,
    
    /// 属于这个进程的所有线程
    pub tasks: SpinLock<Vec<Arc<TCB>>>,
    /// 进程的运行目录
    cwd: SpinLock<Arc<Dentry>>,
    /// 文件系统操作的 umask
    umask: SpinLock<u16>,
    /// 等待这个进程退出的任务
    waiting_task: SpinLock<Vec<Arc<dyn Task>>>,

    /// 信号处理
    signal: Signal,

    /// 子进程
    children: Mutex<Vec<Arc<PCB>>>,

    /// itimer 定时器相关
    pub itimer_ids: SpinLock<[Option<u64>; 3]>,
}
```

`PCB` 结构中，存储了进程的基本信息，包括所属的线程、文件系统信息、信号处理等。

除此之外 KernelX 还使用 `pid` - `PCB` 映射表来快速查找进程：

```rust
// src/kernel/task/manager.rs
static PCBS: SpinLock<BTreeMap<Tid, Arc<PCB>>> = SpinLock::new(BTreeMap::new());

/// 内核启动时创建 init 进程
pub fn create_initprocess(initpath: &str, initcwd: &str, initargs: &str);
/// 获取 init 进程的 PCB
pub fn with_initpcb<F, R>(f: F) -> R
where
    F: FnOnce(&Arc<PCB>) -> R;
/// 管理 PCB 映射表
pub fn insert(pcb: Arc<PCB>);
pub fn get(tid: Tid) -> Option<Arc<PCB>>;
pub fn remove(tid: Tid) -> Option<Arc<PCB>>;
pub fn pcbs() -> &'static SpinLock<BTreeMap<Tid, Arc<PCB>>>;
```

这样，我们就可以方便的在 `kill` 或者 `procfs` 等功能中快速通过 `pid` 找到对应的进程了。

## PCB 状态管理

### PCB 状态

`PCB` 的不是一个 `Task`，它的状态没有 `Task` 那么复杂，它只有三个状态。

```rust
// src/kernel/task/pcb.rs
enum State {
    /// PCB 正在正常运行或者阻塞
    Running,
    /// PCB 已经退出，等待父进程回收
    Exited(u8),
    /// PCB 已经被彻底清理完毕，拒绝被再次回收
    Dead,
}

impl PCB {
    fn recycle(&self) -> Option<u8>;
}
```

`PCB` 的状态不能简单的区分为 `Running` 和 `Exited`，还需要一个 `Dead` 状态来表示这个 `PCB` 已经被彻底清理完毕，这是防止父进程中有多个线程同时调用 `wait` 回收子进程时发生竞态条件。父进程在尝试回收子进程时，应该调用 `recycle` 方法，这个方法会原子性地检查 `PCB` 的状态，如果是 `Exited`，就将状态改为 `Dead` 并返回退出码，否则返回 `None`，来防止竞态。

### PCB 退出

`PCB` 在退出的时候：

1. 将所有子线程标记为 `Exited` 状态，这些子线程在再次调度的时候会被清理掉，并清空子进程列表，释放所有权。

2. 将自己的状态改为 `Exited`，并保存退出码。

3. 如果自己是 `init` 进程，直接引发 `Panic`，因为 `init` 进程不允许退出。

4. 将自己的所有子进程挂载到 `init` 进程下，`init` 进程在设计上会负责回收这些孤儿进程。

5. 唤醒父进程中所有等待回收自己的任务，即父进程中的 `waiting_task: SpinLock<Vec<Arc<dyn Task>>>` 列表中的任务。

6. 把自己从 `PCBS` 映射表中移除，释放所有权。现在，只有父进程和未被回收的子线程还持有对自己的引用。等到父进程回收自己，并且所有子线程退出后，`PCB` 会被彻底清理掉。

### wait 实现

`PCB` 提供了两个函数来实现 wait 的功能：

```rust
// src/kernel/task/pcb.rs
impl PCB {
    /// 等待指定子进程退出
    pub fn wait_child(&self, pid: i32, blocked: bool) -> Result<Option<u8>, Errno>;
    /// 等待任意子进程退出
    pub fn wait_any_child(&self, blocked: bool) -> SysResult<Option<(i32, u8)>>;
}
```

如果 `blocked` 参数为 `true`，表示调用任务愿意阻塞等待子进程退出，否则如果没有子进程退出则立即返回 `Ok(None)`。

在 `blocked` 为 `true` 的情况下，如果没有子进程退出，当前任务会被阻塞并挂起，`PCB` 会将当前正在运行的任务加入到自己的等待列表 `waiting_task: SpinLock<Vec<Arc<dyn Task>>>` 中，等待唤醒。唤醒的事件有两个，分别是正常有子进程退出到达的 `Event::Process(Tid)` 事件，和信号中断导致的唤醒 `Event::Signal` 事件。

父进程在回收的时候，正如上面所说，会调用 `recycle` 方法来防止竞态条件，如果 `recycle` 失败，表示这个子进程已经被其他任务回收掉了，当前任务需要继续等待或者返回 `Errno::ESRCH` 。

## clone、 exec 和退出

### clone 实现

为了实现 `clone` 系统调用， `PCB` 和 `TCB` 实现了如下接口：

```rust
// src/kernel/task/def.rs
pub struct TaskCloneFlags {
    /// Clone 时是否共享文件描述符表
    pub files: bool,
    /// Clone 时是否共享地址空间
    pub vm: bool,
    /// 是否创建线程（而不是进程）
    pub thread: bool,
}

// src/kernel/task/pcb.rs
impl PCB {
    /// 克隆一个新的进程
    /// # Arguments
    /// * `tcb` - 要 clone 的线程
    /// * `userstack` - 新的线程的用户栈地址
    /// * `flags` - 克隆标志
    /// * `tls` - 线程局部存储地址
    pub fn clone_task(
        self: &Arc<Self>, 
        tcb: &TCB, 
        userstack: usize,
        flags: &TaskCloneFlags,
        tls: Option<usize>,
    ) -> Result<Arc<TCB>, Errno>;
}

// src/kernel/task/tcb.rs
impl TCB {
    /// 克隆一个新的线程
    /// # Arguments
    /// * `tid` - 新线程的线程 ID
    /// * `parent` - 新线程所属的进程
    /// * `userstack` - 新线程的用户栈地址
    /// * `flags` - 克隆标志
    /// * `tls` - 线程局部存储地址
    pub fn new_clone(
        &self,
        tid: Tid,
        parent: &Arc<PCB>,
        userstack: usize,
        flags: &TaskCloneFlags,
        tls: Option<usize>,
    ) -> Arc<Self>;
}
```

`PCB` 的 `clone_task` 主要完成了以下工作：

1. 分配一个新的 `Tid` 作为新线程的线程 ID。

2. 根据 `flags.thread` 决定是创建线程还是进程。如果是线程，则新线程和当前进程共享 `PCB`，否则创建一个新的 `PCB`。

3. 调用 `TCB::new_clone` 来创建新的线程。

`TCB` 的 `new_clone` 主要完成了以下工作：

1. 复制自身的用户态上下文，创建一个新的 `UserContext`。

2. 根据 `flags.vm` 决定是共享地址空间还是复制地址空间。如果共享，则新线程和当前线程使用相同的 `AddrSpace`和设置新的栈顶指针，否则调用 `AddrSpace::fork` 来创建一个新的地址空间。

3. 设置新的用户上下文跳过系统调用指令。

4. 根据参数决定是否设置线程局部存储指针和是否复制文件描述符表。

5. 并创建一个新的 `TCB` 实例，并初始化各个字段。

在 `clone` 系统调用中，内核会调用当前任务的 `PCB` 的 `clone_task` 方法来创建新的任务。如果设置了 `VFORK` 标志，内核会调用子 `TCB` 的 `set_parent_waiting_vfork`，当前任务会进入不可中断阻塞，等待新任务调用 `exec` 或者 `exit`，来唤醒自己。

### exec 实现

exec 的行为比较复杂，需要覆盖当前任务的地址空间、文件描述符表、信号处理等信息。我们选择直接新建一个和原来 `TCB` 有一样 `tid` 的 `TCB`，来代替原来的 `TCB`。 `PCB` 和 `TCB` 提供了如下接口来实现 exec：

```rust
// src/kernel/task/pcb.rs
impl PCB {
    /// 执行一个新的可执行文件，替换当前进程的地址空间
    pub fn exec(self: &Arc<Self>, tcb: &TCB, file: Arc<File>, exec_path: String,  argv: &[&str], envp: &[&str]) -> Result<(), Errno>
}
    
// src/kernel/task/tcb.rs
impl TCB {
    pub fn new_exec(&self, file: Arc<File>, argv: &[&str], envp: &[&str]) -> Result<Arc<Self>, Errno>;
}

```

`PCB` 的 `exec` 方法主要完成了以下工作：

1. 调用 `tcb.new_exec` 来创建一个新的 `TCB`。

2. 将所有子线程标记为 `Exited` 状态，并清空子线程列表，释放所有权，将新的 `TCB` 添加到子线程列表中。

3. 更新自己的可执行文件路径，并将第一个子线程加入调度队列。

注意唤醒 `vfork` 父进程的逻辑是在系统调用中完成的。

`TCB` 的 `new_exec` 方法主要完成了以下工作：

1. 检查传入的待执行文件的 `shebang` 行，如果是脚本文件，则解析出解释器路径和参数，递归调用 `new_exec` 来执行解释器。

2. 创建一个新的地址空间 `AddrSpace`，并加载新的可执行文件到地址空间中，设置好用户栈和参数环境变量。

3. 创建一个新的用户态上下文 `UserContext`，并设置好栈顶指针和程序入口地址等状态。对 `fdtable` 执行 `cloexec` 操作，关闭 `FD_CLOEXEC` 标志的文件描述符。

4. 创建一个新的 `TCB` 实例，继承原来的 `tid` 和 `parent`，并初始化各个字段。原来的 `TCB` 的状态在 `PCB::exec` 中会被标记为 `Exited` 并清理掉，这里无需处理。

### TCB 退出

`TCB` 退出的时候，如果设置了 `tid_address`，会将自己的线程 ID 写回到用户空间，尝试唤醒所有等待在此的 Futex，然后将自己的状态标记为 `Exited`。如果自己的 `tid` 和所属 `PCB` 的 `pid` 相同，表示自己是主线程，调用 `PCB` 的退出逻辑，否则直接从调度器中移除自己，等待被清理掉。

## 文件描述符表

文件描述符表 `FDTable` 用于管理进程的文件描述符：

```rust
// src/kernel/task/fdtable.rs
pub struct FDFlags {
    /// 是否设置了 FD_CLOEXEC 标志
    pub cloexec: bool,
}

struct FDItem {
    pub file: Arc<dyn FileOps>,
    pub flags: FDFlags,
}

pub struct FDTable {
    table: Vec<Option<FDItem>>,
    max_fd: usize,
}

impl FDTable {
    /// 创建一个新的文件描述符表
    pub fn new() -> Self;
    /// 获取指定文件描述符对应的文件
    pub fn get(&mut self, fd: usize) -> SysResult<Arc<dyn FileOps>>;
    /// 设置指定文件描述符对应的文件和标志
    pub fn set(&mut self, fd: usize, file: Arc<dyn FileOps>, flags: FDFlags) -> SysResult<()>;
    /// 获取指定文件描述符的标志
    pub fn get_fd_flags(&self, fd: usize) -> SysResult<FDFlags>;
    /// 设置指定文件描述符的标志
    pub fn set_fd_flags(&mut self, fd: usize, flags: FDFlags) -> SysResult<()>;
    /// 分配一个新的文件描述符
    pub fn push(&mut self, file: Arc<dyn FileOps>, flags: FDFlags) -> Result<usize, Errno>;
    /// 关闭指定的文件描述符
    pub fn close(&mut self, fd: usize) -> SysResult<()>;
    /// 克隆文件描述符表，用于 TCB clone
    pub fn fork(&self) -> Self;
    /// 关闭所有设置了 FD_CLOEXEC 标志的文件描述符，用于 exec
    pub fn cloexec(&mut self);
    /// 设置和获取文件描述符表的最大文件描述符数，用于限制进程可打开的文件数
    pub fn set_max_fd(&mut self, max_fd: usize);
    pub fn get_max_fd(&self) -> usize;
}
```
