# 进程间通信和用户态同步

## 匿名管道

KernelX 实现了匿名管道的部分功能，并将管道上层抽象为了文件，实现了 `FileOps`。

```rust
// src/kernel/ipc/pipe/pipe.rs
struct Pipe {
    inner: Arc<PipeInner>,
    meta: Option<Meta>,
    writable: bool,
    blocked: SpinLock<bool>,
}
impl Pipe {
    // 创建一个管道的读写端
    pub fn create(capacity: usize, blocked: bool) -> (Self, Self);
}
```

一个 `Pipe` 表示一个管道的读端或者写端。`Pipe` 内部使用 `PipeInner` 来管理管道的缓冲区和读写操作，同一个管道的读端和写端共享同一个 `PipeInner`。

```rust
// src/kernel/ipc/pipe/inner.rs
impl PipeInner {
    pub fn new(capacity: usize) -> Self;

    /// 读写，支持阻塞
    pub fn read(&self, buf: &mut [u8], blocked: bool) -> SysResult<usize>;
    pub fn write(&self, buf: &[u8], blocked: bool) -> SysResult<usize>;

    /// 事件等待与取消
    pub fn wait_event(&self, waker: usize, event: PollEventSet, writable: bool) -> SysResult<Option<FileEvent>>;
    pub fn wait_event_cancel(&self);

    /// 读写端计数管理
    pub fn increment_writer_count(&self);
    pub fn decrement_writer_count(&self);
}  
```

每个 `PipeInner` 都有一个固定大小的缓冲区，用于存储管道中的数据。`PipeInner` 内部维护了一个读等待队列和一个写等待队列，用于实现阻塞读写操作。每当读或者写操作无法立即完成时，或者有任务调用 `wait_event` 的时候，当前线程会被加入到相应的等待队列中，并在数据可用时被唤醒。在读写操作完成后，`PipeInner` 会检查等待队列，并唤醒相应的线程。当写端被关闭时，`PipeInner` 会更新相应的计数器，并在必要时唤醒等待的线程，例如在所有写端关闭后，读端的阻塞读操作会被唤醒并返回 EOF。

## 信号机制

KernelX 的信号机制由 `PCB` 和 `TCB` 共同实现。每个 `TCB` 都有一个等待信号列表和一个准备接受处理的信号。而 `PCB` 则维护了一个待处理信号队列。

```rust
// src/kernel/ipc/signal/handle.rs
impl TCB {
    pub fn handle_signal(&self);
    pub fn return_from_signal(&self);
    pub fn try_recive_pending_signal(self: &Arc<Self>, pending: PendingSignal) -> bool;
}
impl PCB {
    pub fn send_signal(&self, signum: SignalNum, si_code: SiCode, fields: KSiFields, dest: Option<Tid>) -> SysResult<()>;
}
```

发送信号的最上层是 `PCB::send_signal`，它首先根据 `dest` 参数判断是发送给某个线程还是进程内所有线程，然后调用指定线程或者尝试遍历的所有线程的 `try_recive_pending_signal` 方法，让线程尝试接收信号。如果没有线程能够接收该信号，则将信号加入进程的待处理信号队列中。

`TCB` 的 `try_recive_pending_signal` 方法会根据线程当前是否正在等待指定信号（`TCB::state::signal_to_wait`）、是否还有待处理的信号（`TCB::state::pending_signal`）、信号是否不可忽略和信号是否被自身屏蔽等情况，决定是否接收该信号。返回用户态之前，内核会调用 `try_recive_pending_signal` 方法，让 `TCB` 尝试从进程的待处理信号队列中接收信号，然后调用 `handle_signal`， 这时 `TCB` 进行真正的信号处理步骤。`handle_signal` 会根据线程当前的执行状态，保存现场并构造用户态的信号处理栈，然后修改线程的指令指针，让线程在返回用户态时跳转到信号处理函数。

`try_recive_pending_signal` 在接收到信号的时候，会用事件唤醒该线程，让阻塞的系统调用返回 `EINTR`。

## Futex
 
KernelX 维护了一个内核地址到 Futex 对象的映射表，每个 Futex 对象包含一个等待队列。Futex 的等待和唤醒操作会操作该映射表。

```rust
// src/kernel/usync/futex/futex.rs
struct FutexWaitQueueItem {
    tcb: Arc<dyn Task>,
    bitset: u32,
}

struct Futex {
    kvalue: &'static i32,
    wait_list: LinkedList<FutexWaitQueueItem>,
}

pub struct FutexManager {
    futexes: SpinLock<BTreeMap<usize, SpinLock<Futex>>>,
}
```

Futex 的等待操作会检查用户态地址处的值是否与预期值相等，如果不相等则直接返回错误码。如果相等，则将当前线程加入到 Futex 对象的等待队列中，并阻塞当前线程。唤醒操作会遍历 Futex 对象的等待队列，唤醒所有符合位掩码条件的线程，并将它们从等待队列中移除。

Futex 提供了以下接口用于系统调用层调用:

```rust
// src/kernel/usync/futex/futex.rs
pub fn wait_current(kaddr: usize, expected: i32, bitset: u32) -> SysResult<()>;
pub fn wake(kaddr: usize, num: usize, mask: u32) -> SysResult<usize>;
pub fn requeue(kaddr: usize, kaddr2: usize, num: usize, val: Option<i32>) -> SysResult<usize>;
```
