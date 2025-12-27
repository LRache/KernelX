# 任务、调度器和事件

任务管理是内核的核心。

## 任务抽象

KernelX 对用户任务和内核任务进行了统一抽象，称之为 `Task`。

```rust
// src/kernel/scheduler/task.rs
pub trait Task: Send + Sync {
    fn tid(&self) -> Tid;
    /// 返回内核上下文指针
    fn get_kcontext_ptr(&self) -> *mut arch::KernelContext;
    /// 返回内核栈引用
    fn kstack(&self) -> &KernelStack;
    
    fn run_if_ready(&self) -> bool;
    fn state_running_to_ready(&self) -> bool;

    fn block(&self, reason: &str) -> bool;
    fn block_uninterruptible(&self, reason: &str) -> bool;
    fn unblock(&self);

    fn wakeup(&self, event: Event) -> bool;
    fn wakeup_uninterruptible(&self, event: Event);
    fn take_wakeup_event(&self) -> Option<Event>;
    
    fn tcb(&self) -> &TCB;
}
```

下面我们将介绍这些函数和设计思路。

## 任务上下文

KernelX 为每一个任务都分配了一个内核栈和一个内核上下文，内核上下文结构体由具体的体系结构定义，任务上下文保存寄存器等关键状态，用于在任务调出时保存现场，在任务切换时恢复现场。内核栈用于任务在内核态下的函数调用和局部变量存储，内核栈的所有权归任务所有，当任务销毁的时候，内核栈也会被自动释放。

用户任务的实现 `TCB` 和内核任务的实现 `KThread` 都包含了内核栈和内核上下文。

## 任务调度和状态管理

任务的调度和状态管理是通过 `run_if_ready`、`state_running_to_ready`、`block`、`unblock` 等函数实现的。

一个任务有以下几种状态：

- 就绪（`Ready`）：任务可以被调度执行。
- 运行（`Running`）：任务正在执行。
- 阻塞（`Blocked`）：任务因为等待某个事件而无法执行。
- 不可打断的阻塞（`Uninterruptible Blocked`）：任务因为等待某个关键事件而无法被信号等机制打断。

内核调度器通过调用这些函数来管理任务的状态转换和调度：

1. 每一个 CPU 核心都有一个调度线程，它会从就绪队列中取出一个任务，调用 `run_if_ready` 检查任务是否可以运行，`run_if_ready` 原子性（避免竞态）的检查任务状态是否为就绪，并将其状态改为运行。

2. 将当前 CPU 的上下文切换到该任务的内核上下文，开始执行任务，将当前任务指针记载到 percpu 位置，任务运行的时候能够获取当前任务。

3. 进行任务切换，通过调用体系结构的方法，保存当前调度线程的上下文，恢复即将运行任务的上下文，实现任务切换。

4. 任务内部可以阻塞自身（也只有任务自己可以阻塞自己），调用 `block` 或 `block_uninterruptible`，将任务状态改为阻塞或不可打断的阻塞。阻塞后，任务需要显式地调用内核提供的 `schedule` 接口，让出 CPU，等待被唤醒。

5. 在 `schedule` 逻辑中，内核通过体系结构层保存当前任务的上下文，然后恢复调度线程的上下文，继续调度其他任务，当无任务可调度时，内核会进入等待中断的状态，来降低系统功耗。

内核调度器的代码位于：

```rust
// src/kernel/scheduler/scheduler.rs
pub fn run_tasks(hartid: usize) -> !;
```

这个函数由每个 CPU 核心在初始化完成后调用，负责选择下一个要运行的任务并进行任务切换。

## current 接口

内核使用了一个 percpu 变量记录指向当前 CPU 状态（包括运行在这个 CPU 上的任务和这个核心上的调度线程上下文，用于重新恢复调度线程）的指针，用于任务代码快速访问。


```rust
// src/kernel/scheduler/processor.rs
pub struct Processor {
    hart_id: usize,
    task: *const Arc<dyn Task>,
    idle_kernel_context: arch::KernelContext,
}

// src/kernel/scheduler/current.rs
pub fn processor() -> &'static mut Processor;
pub fn task() -> &'static Arc<dyn Task>;

/// 阻塞当前任务，再次被唤醒的时候返回
pub fn block(reason: &'static str) -> Event;
/// 阻塞当前任务，且不可被信号等打断，唤醒后返回
pub fn block_uninterruptible(reason: &'static str) -> Event;
/// 睡眠当前任务，睡眠结束返回
pub fn sleep(durations: Duration) -> Event;
```

记录的位置是体系结构相关的，例如，在 RISC-V 上，记录在 tp 寄存器中，示例代码：

```rust
// src/arch/riscv/arch.rs
impl ArchTrait for Arch {
    #[inline(always)]
    fn set_percpu_data(data: usize) {
        unsafe { core::arch::asm!("mv tp, {data}", data = in(reg) data) };
    }

    #[inline(always)]
    fn get_percpu_data() -> usize {
        let data: usize;
        unsafe { core::arch::asm!("mv {data}, tp", data = out(reg) data) };
        data
    }
}
```

## 调度器

KernelX 现在采用简单的顺序调度器（Round-Robin Scheduler），每个任务被分配一个时间片，时间片用完后，任务被强制切换出去，调度器选择下一个就绪任务运行。

## 状态和事件机制

### 事件类型

KernelX 使用事件（Event）机制来实现任务的阻塞和唤醒。任务在等待某个事件时会调用 `block` 或 `block_uninterruptible`，并在事件发生时通过 `wakeup` 方法被唤醒。

KernelX 定义了多种事件类型：

```rust
// src/kernel/event/event.rs
pub enum Event {
    /// 文件事件，例如文件描述符可读可写等
    Poll { event: FileEvent, waker: usize },
    /// 设备、管道或者套接字等阻塞读的时候读就绪
    ReadReady,
    /// 设备、管道或者套接字等阻塞写的时候写就绪
    WriteReady,
    /// 全局定时器超时
    Timeout,
    /// Futex 被唤醒
    Futex, 
    /// 子进程状态变化
    Process { child: Tid },
    /// 等待信号的时候收到信号
    WaitSignal { signum: SignalNum },
    /// 收到信号
    Signal,
    /// 唤醒父进程等待的 vfork 任务
    VFork,
}

// src/kernel/event/poll.rs
pub enum FileEvent {
    ReadReady,
    WriteReady,
    Priority,
    HangUp,
}
```

### 阻塞和唤醒

每当一个任务被唤醒，内核会将对应的事件传递给任务，任务可以根据事件类型进行相应的处理。`current::block` 和 `current::block_uninterruptible` 会返回导致任务阻塞的事件，任务可以根据这个事件决定后续的操作。也可以通过 `Task::take_wakeup_event() -> Option<Event>` 获取唤醒时的事件，通常，这个返回值是可以被 `unwrap` 的，如果没有事件，说明任务被错误地唤醒或者存在竞态。设置事件和设置状态是原子性的，防止竞态条件。

KernelX 的 `TCB` 和 `KThread` 在设计上，如果当前任务不在阻塞，但是被使用 `wakeup` 唤醒了，那么 `wakeup` 会返回 `false`，表示没有实际唤醒任务，这个事件也将被丢弃。因此，在将当前任务加入到等待队列之前，必须确保任务处于阻塞状态，或者暂时持有唤醒队列的锁，否则可能会丢失唤醒事件，然后再使用 `current::schedule` 主动引发调度。

KernelX 提供了 `WaitQueue` 来抽象等待队列：

```rust
pub struct WaitQueue<T: Copy>;

impl<T: Copy> WaitQueue<T> {
    /// 创建一个新的等待队列
    pub fn new() -> Self;
    /// 将任务加入等待队列并阻塞
    pub fn wait(&mut self, task: Arc<dyn Task>, arg: T);
    /// 将当前任务加入等待队列并阻塞
    pub fn wait_current(&mut self, arg: T);
    /// 唤醒所有等待队列中的任务
    pub fn wake_all(&mut self, map_arg_to_event: impl Fn(T) -> Event);
    /// 移除指定任务
    pub fn remove(&mut self, task: &Arc<dyn Task>);
}
```

`wait_current` 会在持有锁的情况下，将当前任务加入等待队列并阻塞，但不会主动调度，因此调用者需要在调用后释放锁并主动调用 `current::schedule` 来引发调度。

`wake_all` 函数会唤醒等待队列中的所有任务，并通过传入的闭包将等待参数映射为事件传递给任务。

通常，设备驱动也会使用 `WaitQueue` 来管理等待 I/O 事件的任务，例如串口在有内容到达的时候唤醒所有等待读数据的任务。

### 计时器事件

KernelX 提供了计时器机制，允许任务在等待超时时间到达时被唤醒，事件为 `Event::Timeout`。

计时器的内部维护了一个最小堆，用于高效地管理多个计时器事件，在有时钟事件到达的时候，内核会检查堆顶的计时器事件，如果到达时间，则将对应的任务唤醒。

```rust
// src/kernel/event/timer.rs
/// 添加一个计时器，指定时间后唤醒任务，返回计时器 ID
pub fn add_timer(task: Arc<dyn Task>, time: Duration) -> u64;
/// 添加一个带回调函数的计时器，指定时间后执行回调函数
pub fn add_timer_with_callback(time: Duration, callback: Box<dyn FnOnce()>);
/// 移除指定 ID 的计时器
pub fn remove_timer(timer_id: u64);
```
