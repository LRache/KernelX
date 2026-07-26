# KernelX SMP 竞态与性能问题审计报告

- 审计日期：2026-07-26（分支 `dev`）
- 审计方法：按子系统（klib 同步原语/arch、调度器/任务、内存管理、文件系统、IPC/事件/futex、syscall/驱动）逐一追踪实际代码路径验证，无推测性结论；跨子系统的关键发现均由两处以上独立分析交叉印证。
- 行号基于审计时的工作区状态，修复后可能漂移；每项均给出足以重新定位的符号名。
- 状态标记：`[已修复]` / `[未修复]` / `[部分修复]` / `[无需修复]`，修复详情见文末"六、修复记录"。

## 总体结论

内核最核心的两个 SMP 机制经验证是**正确**的：

1. **两阶段睡眠协议**：任务在仍占用 CPU 时先置 `Blocked`（`TCB::block`，`src/kernel/task/tcb.rs`），跨核唤醒若落在切换完成前，只置 `wake_pending`（`make_ready`）而不入队；`finish_switch` 在 idle 栈上、上下文完全保存后才重新入队。不存在"一个任务同时跑在两个核上"或双重入队的路径。
2. **TLB shootdown 协议**：`active_cpu_mask` 在页表自旋锁下维护，trap 进入用户态前 `deactivate_cpu`、返回前 `activate_cpu` + 本地 `sfence.vma`，远程失效走 SBI RFENCE；"清 PTE → 同步远程 flush → 释放页帧"的顺序在所有 unmap 路径上成立。

同时有一条**未文档化的全局不变量**支撑着现有设计：内核 S-mode 代码全程 SIE=0（中断只在用户态和 idle 循环中开启），因此不关中断的 `SpinLock` 目前不会发生同核中断死锁。**所有打破该不变量的未来改动（内核抢占、长系统调用中开中断等）都会使全部自旋锁变成死锁隐患**，见 M12。

问题集中在机制的组合处：中断上下文误用睡眠锁、"先标记阻塞再注册"的顺序被打破、信号投递与阻塞缺乏原子性。以下按严重程度分类。

---

## 一、严重问题（Critical：可致 panic / 死锁 / 数据损坏）

### C1. on-stack 睡眠锁 unlock 路径对栈上 waiter 的 use-after-free

- **位置**：`src/klib/ksync/sleeplock_onstack.rs`（`SleepLockerOnStack::unlock`，grant 后再读 `waiter.task()`）；`src/klib/ksync/sleep_rwlock_onstack.rs`（`grant_waiters` 的写者授予与读者批量唤醒路径，同型）。
- **机理**：`unlock` 在 waiters 自旋锁内先 `waiter.grant()` 再 `waiter.task()`。而等待方（`lock` 的自旋循环 `while !waiter.is_granted() { schedule() }`）检查 `is_granted()` **不持 waiters 锁**——若 grant 恰落在等待方入队后、首次检查前，等待方立即返回并弹出包含 `WaiterOnStack` 的栈帧继续执行；解锁方随后从已被覆写的栈内存中读取 `Arc<dyn Task>` 胖指针并对垃圾地址做引用计数递增。
- **竞态双方**：hart A 执行 `unlock`（grant 与 task() 之间）；hart B 为自旋检查 granted 的等待者（已置 Blocked 但尚未 schedule）。
- **后果**：内核内存损坏。这两把锁保护 `AddrSpace::map_manager`（`SleepRwLockOnStack`）与每个 `MapArea`（`SleepLockOnStack`）、inode 生命周期锁，窗口被持续高频触发。
- **修复方案**：在 `grant()` **之前**先取出 `waiter.task()` 的克隆（grant 是发布点，发布后不得再触碰 waiter 内存）。
- **状态**：[已修复]（修复记录 #1）

### C2. vfork 父进程 lost-wakeup → 永久不可中断挂死

- **位置**：`src/kernel/syscall/task.rs`（`do_clone` 的 `CloneFlags::VFORK` 分支）；`src/kernel/task/tcb.rs`（`wake_parent_waiting_vfork`）。
- **机理**：父进程先 `child.set_parent_waiting_vfork(...)`、`scheduler::push_task(child)`，**之后**才 `current::block_uninterruptible("vfork")`。子进程可立即在另一核上运行至 `execve`/`exit` 并调用 `wake_parent_waiting_vfork`：该函数 `take()` 消费唤醒槽后调 `wakeup_task_uninterruptible`，而父进程此刻仍是 `Running`，唤醒失败（仅打 warn 日志），唤醒槽却已被清空。
- **后果**：父进程随后进入不可中断阻塞，且再无任何唤醒者——永久挂死，kill 无效。
- **修复方案**：采用"先阻塞后发布"模式：先对当前任务调 TCB 级 `block_uninterruptible`（标记阻塞、不切换），再 `push_task(child)`，最后 `schedule()` 取事件。
- **状态**：[已修复]（修复记录 #4）

### C3. TTY 中断处理持 serial 自旋锁调用 send_signal（IRQ 上下文睡眠 + ABBA 死锁）

- **位置**：`src/driver/char/serial/stty.rs`（`Stty::handle_interrupt`）。
- **机理**：中断处理函数持有 `self.serial`（SpinLock）时对 Ctrl-C 调 `current::pcb().send_signal(...)`；`PCB::send_signal` 会锁 `PCB::tasks`——原为 **SleepLock**。若该锁被占用，`SleepLocker::lock` 会在**中断上下文**中调度走被打断的任务，且此时 `serial` 自旋锁仍被持有、PLIC 中断尚未 complete。
- **竞态双方**：hart A 收 UART RX 中断需要 `pcb.tasks`；hart B 持 `pcb.tasks`（如 `PCB::exit`，其内部 `close_all()` 又可经控制台写取 `serial`）→ **ABBA 死锁**。
- **后果**：全系统控制台死锁；未 complete 的 PLIC 中断使 UART 中断永久失效。即使无 ABBA 配对，中断落在空闲 hart 上时 `SleepLocker::lock` 调 `current::task()`——`Processor::task()` 在 release 模式是 `unwrap_unchecked()` → **未定义行为**。
- **修复方案**：信号投递路径不得使用睡眠锁（见 C4）；`handle_interrupt` 先释放 `serial` 锁再处理输入事件与发信号。
- **状态**：[已修复]（修复记录 #8，与 C4 一并根治）

### C4. POSIX 定时器到期回调在时钟中断上下文经 send_signal 取 SleepLock

- **位置**：`src/kernel/trap.rs`（`timer_interrupt`）→ `src/kernel/event/timer/timer.rs`（`wakeup_expired` 在 IRQ 上下文执行回调）→ `src/kernel/event/posix_timer.rs`（`expire_global_timer` → `notify` → `pcb.send_signal`）→ `src/kernel/ipc/signal/handle.rs`（锁 `PCB::tasks` SleepLock）。
- **机理/后果**：与 C3 同根因——`PCB::tasks` 是 SleepLock 而信号可从 IRQ 上下文发出。锁被占用时 IRQ 内睡眠；发生在空闲 hart 上时 release 模式 UB。
- **修复方案（根治 C3+C4）**：将 `PCB::tasks` 改为 SpinLock；所有持锁期间做睡眠操作的调用点（`PCB::exit`/`exec`/`recycle` 等）改为"锁内克隆/取出快照 → 释放锁 → 锁外执行睡眠操作"；审计 `send_signal` 可达路径全部为 spin-only。
- **状态**：[已修复]（修复记录 #8）

### C5. 信号在目标即将 block() 前送达即丢失（SIGKILL 可被无限期推迟）

- **位置**：`src/kernel/task/tcb.rs`（`TCB::block` 不检查 `state.pending_signal`）；`src/kernel/ipc/signal/handle.rs`（`try_recive_pending_signal` 将 `Err(WakeupFailure::NotBlocked)` 视为投递成功，不重试）。
- **机理**：hart A 发信号：置 `pending_signal` → `wakeup()` 因目标仍 `Running` 而失败 → 视为已送达。hart B 上目标任务正处于阻塞型 syscall 的"进入 ~ block()"之间（pipe read、eventfd、timerfd、futex、epoll_wait、nanosleep、msg/sem 等全部命中），随后带着未处理的 pending signal 睡下，无人再唤醒。上一次信号检查是**上一个** trap 的 `trap_return`，窗口是整个用户态+内核态区间，现实可触发。
- **后果**：wait4/futex/pipe 中的进程对信号（含 SIGKILL）无限期无响应，临时或永久不可杀。
- **修复方案**：`TCB::block` 在同一临界区内检查 `pending_signal.is_some()`，有则拒绝入睡并注入 `Event::Signal`，等待点走既有 EINTR 分支。
- **状态**：[已修复]（修复记录 #5）

### C6. poll/select 在 prepare-to-wait 临界区内获取 SleepLock（陈旧 waiter 表项覆盖唤醒事件）

- **位置**：`src/kernel/syscall/event.rs`（`do_poll`/`select` 先 `tcb.block("poll")` 再逐文件注册）；`src/kernel/ipc/pipe/inner.rs`（`PipeInner::state` 为 SleepLock）；`msgpipe.rs` 同型。
- **机理**：任务已处于 `Blocked`（可中断）状态时注册 pipe/unix socket 需要拿 `PipeInner::state` SleepLock。若锁被占用，任务在 sleeplock 等待队列中以错误状态睡眠；先注册的文件事件此时可将其从 sleeplock 等待中唤醒，任务继续执行并可能再次入睡，等待队列里留下**陈旧表项**。之后 `SleepLocker::unlock` pop 该表项时，`wakeup_uninterruptible` **无条件覆盖**尚未消费的 `wakeup_event`（如 `Event::Poll` → `Event::SleepLock`）。
- **后果**：唤醒丢失（poll 注册已被清空却重新睡眠 → 挂死）；或陈旧 `Event::SleepLock` 打进其他等待点的 `_ => unreachable!()` → **内核 panic**；unlock 唤醒失败时也不尝试下一个等待者，剩余等待者滞留。
- **修复方案**：`PipeInner::state`/`MessagePipeState` 改为 SpinLock（用户态拷贝需移出锁外，改动较大）；`block_task_uninterruptible` 不得忽略 `block_uninterruptible` 的失败返回；`wakeup_uninterruptible` 拒绝覆盖未消费事件；`unlock` 唤醒失败时继续唤醒下一个。
- **状态**：[未修复]（涉及 pipe 读写路径把用户缓冲区拷贝移出状态锁的重构，建议作为独立工作项；H6 的修复已消除其中一类陈旧事件来源，但本项根因仍在）

### C7. shm attach 与 fork/munmap 的 ABBA 死锁（自旋锁内睡眠）

- **位置**：`src/kernel/ipc/shm/manager.rs`（`ShmManager::attach` 全程持 `SHM_MANAGER` SpinLock，其内调 `addrspace.with_map_manager_mut` → `map_manager.write()` SleepRwLock）；反序：`src/kernel/mm/maparea/shm.rs`（`ShmArea::fork`/`Drop` 在持 `map_manager.write()` 时回调 `SHM_MANAGER.lock()`）。
- **竞态双方**：hart A 线程 `shmat`：持 SHM_MANAGER（自旋），睡等 `map_manager.write()`；hart B 同进程另一线程 fork/munmap/exit：持 `map_manager.write()`，永久自旋 SHM_MANAGER。
- **后果**：硬死锁；即使无对向竞争，自旋锁内阻塞于睡眠锁在 `spinlock-check` 特性下直接 panic。
- **修复方案**：attach 拆为"锁内校验+预约 → 锁外映射 → 回锁提交/回滚"三阶段。
- **状态**：[已修复]（修复记录 #7）

### C8. `Dentry::get_inode()` 在 inode 加载失败时 panic（用户可触发）

- **位置**：`src/fs/vfs/dentry.rs`：`vfs().load_inode(...).expect("Failed to load inode")`。
- **机理**：hart A 持有 `Arc<Dentry>`（cwd、O_PATH fd、`/proc/<pid>` 目录等）调用任何需要 inode 的操作；hart B (1) 使对应任务退出——procfs 的 `get_inode` 返回 `Err(ENOENT)`；或 (2) unlink 该文件且 inode 缓存已淘汰弱引用，强制重新 `load_inode` 失败。
- **后果**：`ls /proc/<pid>` 与任务退出竞争即可让用户态触发内核 panic。
- **修复方案**：`get_inode` 改为返回 `SysResult<Arc<Inode>>` 并在全部调用点传播错误。
- **状态**：[已修复]（修复记录 #2）

### C9. unlink 立即释放磁盘 inode，无视仍打开的 fd（跨文件数据损坏）

- **位置**：`src/fs/ext4/inode.rs`（`unlink_from_parent`：`inode_nlink == 0` 即 `free_unlinked_inode` = truncate(0)+`ext4_fs_free_inode`）；`src/fs/vfs/dentry.rs`（`remove_child` 无打开引用检查）。
- **机理**：hart A 对打开的 fd 读写；hart B `unlink()` 同一文件（经典 `open; unlink; use` 临时文件模式）。unlink 返回后 A 的下一次 `readat/writeat` 以已释放的 ino 调 `read_inode_ref`；ext4 将该 ino 重新分配给新文件后，A 的写入**破坏新文件的数据/元数据**。
- **后果**：静默跨文件数据损坏 + 违反 POSIX（unlink 后已打开文件必须仍有效）。`ext4_native` 后端设有 `deleted` 标志将后续 I/O 转为 EIO，仍违反 POSIX 但不损坏数据。
- **修复方案**：unlink 时仅标记 orphan，将释放延迟到最后一个 `Arc<Inode>` drop 时执行。
- **状态**：[已修复]（修复记录 #3）

---

## 二、高危问题（High）

### H1. fork 无视在途内核写 pin → syscall 写入静默丢失

- **位置**：`src/kernel/mm/addrspace.rs`（`translate_write` 返回 `WriteChunk` 前释放 manager 读锁）；各 `Area::fork`（`maparea/anonymous/private.rs`、`userstack.rs`、`filemap/private.rs`、`elf.rs`）标 CoW 时不检查 `pins`。
- **机理**：线程 T1（hart A）在 `read(fd, buf)` 中持有指向页 P 的 pin 即将 memcpy；T2（hart B，同进程）`fork()` 将 P 转为 CoW；父进程先写故障 P 拿到新副本，旧帧归子进程——T1 的内核写落入旧帧，父进程缓冲区数据**静默缺失**。即 Linux 以 pin-aware fork（`page_needs_cow_for_dma`）修复的同类 bug。no-swap 构建（`swappable/nofile/noswap.rs`）无 pin 计数，修复所需信息不存在。
- **修复方案**：两种构建都维护 pin 计数，`Area::fork` 对 `pins > 0` 的帧改为立即复制给子进程；或让 `with_translated_*` 在闭包期间持 manager 读锁（不能覆盖长生命周期 pin）。
- **状态**：[未修复]（需先为 no-swap 构建补 pin 计数基础设施）

### H2. execve 不唤醒也不等待兄弟线程

- **位置**：`src/kernel/task/pcb/lifecycle.rs`（`PCB::exec` drain 循环 vs `PCB::exit` 的唤醒序列）。
- **后果**：阻塞中的兄弟线程永不醒来 → 其 TCB、内核栈、**旧地址空间整体永久泄漏**，且滞留在各等待队列中；运行中的兄弟线程与新镜像并发执行旧代码；两个线程并发 exec 会各自 drain 并各推一个 leader。
- **修复方案**：复刻 `PCB::exit` 的唤醒序列；完整方案还应实现 de_thread 屏障（等全部兄弟线程 Exited 后再提交新镜像）并按进程互斥 exec。
- **状态**：[部分修复]（修复记录 #6：泄漏与永不唤醒已修复，drain+推 leader 已并入单临界区消除空窗口；de_thread 式"等待兄弟线程退出后再提交新镜像"的完整屏障未实现，运行中兄弟线程与新镜像短暂并发的窗口仍在）

### H3. 目录 lookup 与 unlink/rename 无 per-directory 串行化（TOCTOU）

- **位置**：`src/fs/vfs/dentry.rs`（`lookup_with_perm` vs `remove_child` / `rename`）。
- **机理**：无锁覆盖"FS lookup → load_inode → 插入 children"对抗"FS unlink → children.remove → cache.remove"的复合序列。交错后：已释放的 ino 被重新 load 进 inode 缓存（ino 复用后**串文件**）；children 中留下指向不存在名字的正向 dentry；`rename` 在 FS 层改名与 dentry 修正之间还有一个返回陈旧 dentry 的窗口。
- **修复方案**：per-Dentry 目录变更锁，覆盖 {FS 操作 + children + inode 缓存修正}；unlink 先摘 children 再释放。
- **状态**：[未修复]（注：C9 修复后 ino 在最后一个引用释放前不会被磁盘复用，"串文件"的最恶性后果已被大幅缓解，但陈旧 dentry/缓存不一致仍在）

### H4. umount 的 busy 检查与新 open 竞争（对已 fini 的 lwext4 状态做 I/O）

- **位置**：`src/fs/vfs/mount.rs`（`unmount`：`superblock_busy` 检查与 `superblock_table.unmount` 之间无互斥）；`src/fs/ext4/superblock.rs`（`ext4_fs_fini`，且 `Drop for SuperBlockInner` 会二次 fini）。
- **修复方案**：先标记 mount 为 dying（拒绝新 lookup），在 mounts 锁下复验 busy，`ext4_fs_fini` 只由 `SuperBlockInner::drop` 调用。
- **状态**：[未修复]

### H5. 共享 futex 以物理页帧为 key 且不持 pin（swap 下永久睡死）

- **位置**：`src/kernel/syscall/futex.rs`（`futex_key`：`PinPageFrame` 在语句结束即释放）；`src/kernel/usync/futex/futex.rs`（`FutexKey::Shared { page: frame.kpage(), .. }`）。
- **机理**：等待者以帧 P 为 key 睡眠；kswapd 换出该页；唤醒方将其换入到不同帧 Q，以 Q 为 key 唤醒——P 上的等待者永不醒来。仅 `swap-memory` 构建可触发，但 key 设计本身错误。
- **修复方案**：共享 futex 改以稳定身份（映射对象 + 页内偏移 / inode + offset）为 key，或在整个等待期间持 pin。
- **状态**：[未修复]

### H6. poll/select 唤醒后不取消"唤醒者"的注册（unix socket 双队列陈旧注册）

- **位置**：`src/kernel/syscall/event.rs`（唤醒后取消所有文件的注册但**跳过 waker**）；unixsocket 的 `wait_event` 同时注册 rx/tx 两个队列，`wake_all` 只清点火的那个。
- **后果**：残留注册向已在别处睡眠的任务投递陈旧 `Event::Poll` → 各等待点 `unreachable!()` panic，或下次 poll 中陈旧 waker 索引错配 fd / 越界。
- **修复方案**：poll/select 的取消路径对**包括 waker 在内**的全部文件调用 `wait_event_cancel()`（幂等）。
- **状态**：[已修复]（修复记录 #6）

### H7. remove_timer 无法拦截已弹出的在途回调（陈旧 Event::Timeout）

- **位置**：`src/kernel/event/timer/timer.rs`（`wakeup_expired` 弹出后释放锁再执行回调；`remove` 对已弹出者无效，也无"等回调完成"语义）。
- **机理**：任务被 futex_wake/信号唤醒后调 `remove_timer`（no-op），另一核的 tick 处理器已弹出该表项，在任务重新进入**另一个**可中断等待后投递 `Event::Timeout`。
- **后果**：无超时的等待收到假 `ETIMEDOUT`/`EAGAIN`，或命中 `unreachable!()` panic。timerfd/posix-timer 有序号防护；所有裸 `add_timer(task, ...)` 用户暴露。
- **修复方案**：给任务唤醒型定时器加 generation/cookie（在任务状态锁下比对后才投递），或 `remove()` 自旋等待在途回调完成。
- **状态**：[未修复]

### H8. sigsuspend/ppoll/pselect 换 mask 与发送方读旧 mask 的丢唤醒

- **位置**：`src/kernel/ipc/signal/handle.rs`（masked 分支不唤醒，仅入 PCB pending 队列）；`src/kernel/syscall/ipc.rs` / `event.rs`（"换 mask → 查队列 → block"序列不原子）。
- **机理**：发送方读到旧 mask（信号被屏蔽）→ 决定不唤醒；等待方换入解除屏蔽的 mask、查队列（尚空）、入睡；发送方随后 `add_pending`——无人唤醒。`sigtimedwait` 因 `signal_to_wait` 协议在 TCB 状态锁下串行而免疫。
- **修复方案**：将 mask 读取+入队与 mask 交换+查队列统一到 TCB 状态锁上（照抄 `signal_to_wait` 协议）。
- **状态**：[未修复]

### H9. 进程凭证按字段独立加锁：撕裂读与检查-后-使用竞态（安全相关）

- **位置**：`src/kernel/task/pcb/mod.rs`（uid/euid/suid/fsuid/gid/… 每字段一把 SpinLock）；`src/kernel/syscall/uid.rs`（`setuid` = 1 次读锁判断 + 4-5 次独立写锁）。
- **后果**：观察者可见"新 euid + 旧 suid"等不可能组合；权限检查（如 `can_access_pidfd_target` 6 次独立加锁读 6 个 uid）可与并发降权交错——检查时是 root、动作时已不是。
- **修复方案**：凭证合并为单 `Credentials` 结构，单锁或 `Arc<Credentials>` 原子整体替换（Linux commit_creds 模式），syscall 先构造新副本再一次性提交。
- **状态**：[未修复]

### H10. `arch::write_volatile` 的 MMIO 屏障方向错误（真实硬件 DMA 乱序）

- **位置**：`src/arch/riscv/arch.rs`（`fence w, i` + `write_volatile`）。
- **机理**：`fence w, i` 排序"先前写 → 后续设备**读**"，而随后的 MMIO store 是设备**写**（O），不在后继集合内——DMA 描述符写与 doorbell 写在弱序硬件上可乱序。应为 `fence w, o`。（`read_volatile` 的 `fence i, r` 正确。）QEMU TCG 下无症状。
- **状态**：[已修复]（修复记录 #1）

### H11. virtio-blk 完成路径出错时跳过 wake_next()（唤醒链断裂）

- **位置**：`src/driver/block/virtio.rs`（`complete_result.map_err(|_| ())?` 提前返回，跳过 `self.wake_next()`；读写两处同型）。
- **机理**：中断只唤醒 used-ring 头部等待者，其余靠完成者逐个接力 `wake_next()`；错误提前返回切断链条，下一个等待者以不可中断状态睡死（中断已 ack，不会再来）。
- **修复方案**：`wake_next()` 移到错误传播之前。
- **状态**：[已修复]（修复记录 #7）

---

## 三、中低危问题（Medium / Low）

### M1. PLIC claim=0 被当作真实 IRQ 分发

- **位置**：`src/arch/riscv/plic.rs`（`claim_irq` 将 claim 寄存器读到 0——"无待处理中断"，SMP 下另一核已 claim 时的常态——映射为 `Some(0)`）。
- **后果**：每次竞争产生一次假分发 + `No driver registered for interrupt 0` 警告 + 无意义的 complete(0) 写。
- **状态**：[已修复]（修复记录 #1）

### M2. Ctrl-C 信号目标错误且信号值错误

- **位置**：`src/driver/char/serial/stty.rs`（`current::pcb()` = 被中断 hart 上恰好在跑的任意进程；空闲 hart 则直接丢弃；且 VINTR 发的是 SIGQUIT 而非 SIGINT，无前台进程组概念）。
- **修复方案**：TTY 维护前台 pgrp（或至少 session leader），经任务管理器定向发 SIGINT，与中断落点解耦。
- **状态**：[未修复]（C3 修复只消除了锁风险，目标选择语义原样保留）

### M3. O_APPEND 跨独立打开的 fd 不原子

- **位置**：`src/fs/file/file.rs`（`write`：先读 `inode.size()` 再 `writeat`，`io_lock` 只在 `writeat` 内）。
- **修复方案**：append 偏移解析下沉到 `Inode::writeat` 内、在 `io_lock` 下完成。
- **状态**：[未修复]

### M4. sendfile/copy_file_range 对共享文件偏移的非原子读-改-写

- **位置**：`src/kernel/syscall/fs.rs`（快照 offset → 循环 pread → 结尾绝对 seek）。
- **状态**：[未修复]

### M5. fcntl(F_SETFL) 非原子且错误改写 FD flags

- **位置**：`src/kernel/syscall/fs.rs`。
- **状态**：[未修复]

### M6. lwext4 create 错误路径在 put_inode_ref 之后 free_inode（use-after-put）

- **位置**：`src/fs/ext4/inode.rs`（`create` 失败分支）。ENOSPC 时可损坏共享 bcache。
- **状态**：[已修复]（修复记录 #3）

### M7. ext4_native 缓存 I/O 路径 expect panic

- **位置**：`src/fs/ext4_native/inode.rs`（`ensure_page().expect(...)`；swap 构建下换入失败/内存压力可触发）。
- **状态**：[已修复]（修复记录 #3）

### M8. `get_time_us` 乘法溢出（约 21 天后时间回绕）

- **位置**：`src/arch/riscv/arch.rs`。
- **状态**：[已修复]（修复记录 #1）

### M9. CPU 特性表按 FDT 顺序而非 hart id 索引

- **位置**：`src/arch/riscv/cpu.rs`（影响 FPU 保存/恢复与 Svadu 判定）。
- **状态**：[已修复]（修复记录 #1）

### M10. `current::task()` 返回指向调度循环栈槽的 `'static` 引用（潜在不健全）

- **位置**：`src/kernel/scheduler/processor.rs` / `current.rs`。跨 `schedule()` 持有会悬垂或别名到另一任务的 Arc。现有调用点均在调度后重取（已逐一核对），但类型签名不阻止误用。另 `processor() -> &'static mut Processor` 与 `run_tasks` 中活跃的 `&mut` 别名，按 Rust 规则是 UB（per-hart 实际无害）。
- **修复方案**：按值返回克隆的 Arc，或收窄生命周期；processor 改裸指针封装/内部可变性。
- **状态**：[未修复]

### M11. 若干进程级共享状态的非原子 RMW / 分离锁

- `umask`（读、写两段锁）；`setsid`/`setpgid`（检查与设置分离）；`{root, cwd}` 两把锁分别读，chroot+chdir 并发可得错配对。
- **状态**：[未修复]

### M12. SpinLock 无 irq-save 变体，安全性依赖未文档化全局不变量

- **位置**：`src/klib/ksync/spinlock.rs`。今天安全（内核态恒 SIE=0），未来任何开中断的改动都会静默引入同核中断死锁。
- **修复方案**：增加 `SpinLockIrq` 变体，并在 `SpinLocker::lock` 中 debug 断言 `sstatus.SIE == 0`。
- **状态**：[未修复]

### M13. 其他小项

- fork 的单次延迟 TLB flush 允许兄弟线程的快照后写入漏进子进程（低危，Linux dup_mmap 历史同款）。[未修复]
- CoW 文件映射错误路径泄漏已入 LRU 的页（当前不可达；匿名页孪生代码正确）。[未修复]
- watchdog 注册在 block 之后、锁外，跨核唤醒可先行 remove → 陈旧表项永久持 Arc（feature `watchdog`）。[未修复]
- `PCB::exit` 在兄弟线程仍运行时关闭其 fd 表（无 UAF，但语义早了）。[未修复]
- ls7a RTC 32 位计数 36 小时不读则丢 wrap。[未修复]
- dentry `children` map 的死 Weak 永不清理（高频目录无界增长）。[未修复]
- `StrongArc::get_mut` 唯一性检查用 Relaxed（应 Acquire；当前无调用者）。[未修复]
- `PhysPageFrame::slice(&self) -> &mut [u8]` 从共享引用铸造可变引用（别名 UB 模式，当前调用被串行化）。[未修复]
- 非 Svadu 路径对只读 CoW PTE 预置 D 位（脏页高估，swap 多写回；正确性无损）。[未修复]

### M14.（横切纪律）所有等待点的 `_ => unreachable!()`

几乎每个等待点（futex、nanosleep、pipe、eventfd、timerfd、msgpipe、poll、ipc sem/msg）对意外唤醒事件 panic。C6/H7 等仍可触发。在事件源修完之前，统一改为"视为 spurious wakeup：重查条件并重新睡眠"，可把整类 bug 从 panic/挂死降级为无害重试——这也是通用正确纪律。
- **状态**：[未修复]（H6 修复后 poll 类陈旧事件来源已消除一大类）

---

## 四、性能问题（按预期收益排序）

### P1. 每次 trap 返回/上下文切换的全量 TLB 冲刷，无 ASID —— 预计两位数百分比收益

- `clib/src/arch/riscv/trap/usertrap.S`：每次返回用户态 2 次全量 `sfence.vma`（含清 global 项）+ 重写 satp；`clib/src/arch/riscv/swtich.S`：每次 `kernel_switch` 再来 2 次，且 satp 未变（内核线程、同进程线程、task↔idle）也不跳过。
- **方案**：satp 未变则免 fence；引入 ASID；或 per-hart "欠 flush" 标志（远程 flush 跳过未激活 hart 时置位，返回用户态时才补）。
- **状态**：[已修复]（优化记录 #A：stale-mask 延迟冲刷 + satp 比较跳过；未采用 ASID）

### P2. 单页 PTE 更新触发全地址空间远程 shootdown，且持页表锁发 SBI

- `src/arch/riscv/pagetable/pagetable.rs`：`mmap_raw`/`unmap`/protect 每次调 `flush_tlb()`，经 `sbi.rs` 传 start=0,size=0 = 全量。首次缺页安装（invalid→valid）**不需要任何 shootdown**却也发；远程 fence 在页表自旋锁内发出，而 `activate_cpu/deactivate_cpu` 每次 trap 出入都要取同一把锁。
- **方案**：invalid→valid 不发远程 fence；其余传实际 (vaddr, len)；掩码快照后锁外发 SBI。
- **状态**：[已修复]（优化记录 #A：invalid→valid 仅本地单页 fence；失效路径范围化 fence（≤32 页）；SBI 出锁仅在 swap 延迟路径自然成立处实施，per-PTE 路径仍在锁内——可接受，因其余优化已大幅减少发 SBI 的次数）

### P3. 无调度 IPI + 全局单就绪队列 + 空队列也全切换

- 无任何 `send_ipi`（riscv 后端仅有 RFENCE/HSM）。唤醒落在 `wfi` 空闲核最多等 10ms tick；全局 `spin::Mutex<VecDeque>` 就绪队列每次调度/唤醒全核争抢，无 per-hart 队列、无亲和性；每 tick 无条件 `schedule()`：即使队列空也做 2 次 `kernel_switch` + 4 次锁往返。
- **方案**：`push_task` 时 SBI IPI 踢一个空闲核；per-hart 运行队列 + 空闲窃取；tick 中队列空则跳过调度。
- **状态**：[部分修复]（优化记录 #D：IPI 踢空闲核 + 空队列 tick 跳过已实现；per-hart 运行队列/亲和性未做。注：曾怀疑 idle 循环在 SIE=1 下取就绪队列锁存在同核死锁窗口，经核实 `run_tasks` 每轮循环首句即 `disable_interrupt()`，该窗口不存在）

### P4. 每次 syscall 进出 6+ 次锁操作的时间记账

- `src/kernel/trap.rs`（trap_enter/trap_return）：2×`timer::now()` + 2×`time_counter` SpinLock + 2×`PCB::tasks_time_usage`（**全进程共享锁**）+ `check_cpu_timers` 两次锁并在有定时器时 `collect()` 分配。
- **方案**：时间按线程本地累计，切换/退出时折叠进 PCB；per-PCB 原子标志"有 CPU 定时器"跳过 `check_cpu_timers`。
- **状态**：[已修复]（优化记录 #C）

### P5. futex：全局唯一 SleepLock，锁内读用户字可缺页

- `src/kernel/usync/futex/futex.rs`：全系统 futex 串行在一把 SleepLock 上；用户字比较在锁内做，CoW/换入时在锁内做分配/IO；`cancel_wait_all` 每次超时/EINTR 全表扫描。pthread 锁扩展性塌到单核。
- **方案**：按 `FutexKey` 哈希分桶 SpinLock（Linux 模式）；桶锁内用非缺页访问读用户字。
- **状态**：[已修复]（第二轮优化：64 桶哈希 SpinLock；锁外 translate/pin 用户页 + 锁内经 pin 映射免缺页复查；waiter 携带 `location: Arc<SpinLock<FutexKey>>`（Linux `futex_q->lock_ptr` 模式）使 requeue 与 cancel 竞争安全；waitv 按序取全部涉及桶锁保持原子性；cancel 免全表扫描。顺带：pin 现全程存活于等待期间，H5 的"pin 提前释放"半项修复——key 仍按物理页，H5 的 key 身份问题在 swap 启用前保持开放）

### P6. 文件系统锁粒度与刷盘放大

- 每 FS 一把全局 `SleepLock<SuperBlockInner>` 且 `dev_bread/bwrite` 在锁内——吞吐≈单核；vfat/exfat 无页缓存，数据读全串行。
- 每次 unlink 和每次 inode 缓存淘汰都触发 `SuperBlockInner::flush()` = 整个 bcache 刷盘 + 设备 barrier（`rm -rf`/untar 的最大放大器）；`Ext4Inode::drop` 再来一次。
- `io_lock` 覆盖整个 `readat` 含页缓存命中——热文件无读并行。
- 无 readahead；`sendfile`/`copy_file_range` 用 1KiB 栈弹跳缓冲。
- `size()`/`fstat()` 走全局锁而 `cached_size` 已存在。
- **方案**：设备 I/O 移出 sb 锁；per-inode 回写、sync 时才全量 flush；io_lock 改读写锁；64-256KiB 簇式预读；sendfile ≥64KiB 堆缓冲；fstat 用 cached_size。
- **状态**：[部分修复]（优化记录 #B：刷盘放大、lookup atime、size() 免锁已修复；sb 锁内设备 I/O、io_lock 读并行、readahead、sendfile 缓冲仍未做）

### P7. gettimeofday：全局 SpinLock + 每次 2 个 MMIO（QEMU 下 VM exit）

- `src/driver/chosen/kclock.rs`。**方案**：启动时记 RTC 偏移，REALTIME = 偏移 + monotonic CSR，热路径免锁免 MMIO，顺带为 vDSO 时间铺路。
- **状态**：[已修复]（优化记录 #C）

### P8. 内存管理杂项

- CoW/缺页路径先 `alloc_with_shrink_zeroed()` 再整页 `copy_from_slice`——每次白付 4KiB memset。[已修复]（优化记录 #C）
- 全局堆分配器单锁（TLSF）与全局页帧分配器单锁：建议 per-CPU 缓存/批量补充。[未修复]
- per-area 睡眠锁把大 mmap 并发缺页串行化，且 backing I/O 在锁内。[未修复]
- 线程创建/销毁的内核栈映射走 `flush_tlb_all()`（本地+SBI 远程全量）且持 `KERNEL_PAGETABLE` 锁——创建（原本未映射）根本不需要 flush。[已修复]（第二轮优化：创建走 `map_fresh_kernel_pages` 仅本地单页 sfence；销毁保留 shootdown 但范围化且 SBI 移出锁外；已验证 VA 复用前销毁冲刷同步完成，其余 map_kernel_pages 调用点（权限翻转/MMIO 重映射）逐一审计保留全量路径）
- kswapd 为 500ms 轮询而非水位唤醒；brk 每次增长新建 area 不合并；无用户态大页。[未修复]

### P9. 同步/IPC 杂项

- 自旋锁无 test-and-test-and-set、无 `core::hint::spin_loop()`——争用时缓存行乒乓最大化。[已修复]（修复记录 #1：`spinlock.rs` 与 `rwlock.rs` 均改为先只读自旋+relax 再 CAS）
- pipe：每次读/写唤醒**全部**对端等待者（惊群）；内核缓冲路径逐字节 `push_back`（[已修复]——第二轮优化：改为最多两段的批量 slice 拷贝）；`PipeState` 是 SleepLock。[部分修复]
- epoll 对 level-triggered fd 每次 `epoll_wait` 做 2-3 遍 O(n) 全量轮询，退化为 poll。[未修复]
- 全局定时器：单锁 BinaryHeap，每 hart 每 tick 都扫，`remove` 是 O(n) retain 重建，每个 sleep 一次 Box 分配；无按最早到期重编程。[未修复]
- 日志：`KLOG_LOCK` 自旋锁内逐字节 SBI ecall；`print!`/panic 路径绕过行锁交错输出。UART `write` 持锁忙等逐字节 + 每字节取一次 attr 锁。[未修复]
- `getrandom`/`syslog` 按用户可控长度一次性 `vec![0; len]` 分配。[未修复]
- PLIC claim/complete 走全局锁，且中断广播到所有核。[未修复]

---

## 五、经验证正确、无需改动的设计

- 两阶段睡眠协议与 `wake_pending`/`finish_switch`（见总体结论）。
- TLB shootdown 协议与 swap 的 `pins` + `tlb_flush_pending` + `TlbInvalidationLock` 协议；swap-in 单次读；CoW 同进程并发由 per-area 锁串行，父子并发最坏双拷贝（良性）；`mapping_refs()==1` 原地复用与 fork 增计数互斥于 manager 写锁。
- lwext4 FFI：所有 C 调用都在 per-superblock `SleepLock` 内，C 侧状态 per-superblock 装箱，无跨 mount 共享。
- fd 表：`get()` 克隆 Arc，close 与在途 read 无 UAF；fork/cloexec/close_range 正确配对 install/remove 回调。
- 共享 fd 偏移：`RandomAccessFile::pos` SleepLock 覆盖整个 I/O；pread/pwrite 不触碰。
- pipe 睡眠注册在状态锁内先 block 后释放——无 lost-wakeup（其 SleepLock 类型问题见 C6）。
- inode 缓存 Loading/Syncing 哨兵 + 等待队列正确关闭 lookup-vs-eviction UAF。
- ext4_native / memtreefs 跨目录 rename 按 ino 排序双亲锁——无 AB-BA。
- uptr / IOVec：整结构一次拷入内核后使用，无 double-fetch；用户拷贝经软件页表翻译 + pin 帧，无"拷贝中缺页"路径，并发 munmap 安全。
- 驱动探测、chosen 单例全部在 SMP 启动前单线程完成；`InitedCell` CAS 门控 + Release 发布 + Acquire 读取正确；各锁原语内存序（Acquire/Release）正确；`StrongArc::drop` 为标准 Release + Acquire fence 模式。
- TTY 读路径：waiters 先于条件检查加锁并贯穿 `wait_current`——无 lost-wakeup；`line → input` 锁序一致。
- virtio-blk 提交/等待协议（block → 登记 inflight → 复查 used → schedule）正确关闭丢唤醒窗口（错误路径问题见 H11，已修复）。
- 调度器 idle 循环：`run_tasks` 每轮循环首句 `disable_interrupt()`，SIE=1 的窗口（`enable_interrupt` ~ `wfi`）内不取任何锁——无同核中断死锁窗口。

---

## 六、修复记录

修复日期：2026-07-26。全部修复通过 `make check`（0 错误）与 `make kernel` 完整构建链接（生成 `build/riscv64/vmkernelx`）；剩余 77 条警告均为修复前已存在。

### #1 klib/arch 层小型修复（C1、H10、M1、M8、M9、P2 部分、P9 部分）

- **C1**：`src/klib/ksync/sleeplock_onstack.rs`（`SleepLockerOnStack::unlock`）与 `src/klib/ksync/sleep_rwlock_onstack.rs`（`grant_waiters` 的写者授予路径与读者批量唤醒循环）：将 `waiter.task()` 的克隆提前到 `waiter.grant()` **之前**。grant 是发布点，发布后等待方可立即返回并弹栈；已 grep 确认全仓仅此三处 grant 调用点，grant 后不再触碰 waiter 内存。
- **H10**：`src/arch/riscv/arch.rs` `write_volatile`：MMIO store 前屏障 `fence w, i` → `fence w, o`；`read_volatile` 的 `fence i, r` 保持不变。
- **M1**：`src/arch/riscv/plic.rs` `claim_irq`：追加 `.filter(|&irq| irq != 0)`，claim=0（无待处理/已被他核 claim）返回 `None`，不再按 IRQ 0 分发。
- **M8**：`src/arch/riscv/arch.rs` `get_time_us`：改写为 `t / f * 1_000_000 + (t % f) * 1_000_000 / f`，消除 u64 乘法溢出。
- **M9**：`src/arch/riscv/cpu.rs`：`CPU_INFO` 改存 `Vec<(hart_id, CPUInfo)>`（以 FDT `reg` 值为 key），`get_cpu_info(hart_id)` 按 id 匹配查找；签名与 `core_count()` 语义不变。
- **P2 部分**：`src/kernel/trap.rs`：`assert_user_satp_matches_addrspace` 及其唯一调用点整体加 `#[cfg(all(target_arch = "riscv64", debug_assertions))]`，release 构建不再每次 trap 返回都取页表锁读 satp。
- **P9 部分**：`src/klib/ksync/spinlock.rs`（`SpinLocker::lock`）与 `src/klib/ksync/rwlock.rs`（读/写获取路径）：CAS 失败后先以 Relaxed load 只读自旋（带 `core::hint::spin_loop()`）至锁看似可得再重试 CAS；CAS 本身内存序与 spinlock-check 插桩不变。
- 另核实一项疑点后**未改动**：idle 循环在 `wfi` 返回后 SIE=1 的窗口内不取任何锁（`run_tasks` 每轮循环首句即 `disable_interrupt()`），无死锁窗口。

### #2 `Dentry::get_inode()` 去 panic 化（C8）

- `src/fs/vfs/dentry.rs`：`get_inode` 返回类型改为 `SysResult<Arc<Inode>>`，`load_inode` 失败以 `?` 传播（工作区中已有的改造开头被保留并补完）。
- 补完 18 处编译中断的调用点：17 处在 `src/kernel/syscall/fs.rs`（do_openat、openat2、faccessat、fanotify_mark、fstatat、statx、utimensat、do_chmod、do_chown、truncate64、mount 块设备查找等），全部 `?` 传播；唯一非传播点为 `Dentry::bind_mount`（本身不可失败）——改为克隆源 dentry 缓存的 `Weak<Inode>`，陈旧 weak 无害（`get_inode` 会惰性重载）。全路径未引入新的 unwrap/expect。

### #3 ext4 orphan inode 延迟释放（C9）+ M6 + M7

- **C9**：`src/fs/ext4/superblock.rs`：`SuperBlockInner` 新增 `unlinked_inos` orphan 列表及 `mark_unlinked(ino)` / `take_unlinked(ino)`（全部在既有 per-superblock 锁内变更）。放在 superblock 而非 `Ext4Inode` 字段，是因为 `unlink_from_parent` 在**父** inode 上执行，只知道子 ino、拿不到子的内存 inode 对象。`src/fs/ext4/inode.rs`：`unlink_from_parent` 在 nlink 归零时（普通文件与 rmdir 分支，rename 覆盖路径共用该助手）只做标记、不再立即 `free_unlinked_inode`，也不再在 unlink 时刻失效 inode-ref 缓存；`Drop for Ext4Inode` 消费标记并调用新增的 `free_unlinked()`——读 ref、truncate(0)+`ext4_fs_free_inode`、put ref、失效缓存、flush，单次 superblock 锁内完成，并跳过该文件的常规脏页回写。dentry/inode 缓存仍在 unlink 时刻移除（新 open 无法再到达）；缓存 `Cache::remove` 释放强 Arc 后，最后一个 fd 关闭触发最终 Drop 与延迟释放；期间 ino 在磁盘上保持已分配，不会被 `ext4_fs_alloc_inode` 复用。unmount 时序防护沿用既有 Drop 的模式（其无卸载防护的问题即 H4，另行处理）。
- **M6**：`src/fs/ext4/inode.rs` `create` 错误分支：改为先 `ext4_fs_free_inode` 后 `put_inode_ref`（与同文件 unlink 路径的既有顺序一致），错误传播语义不变。
- **M7**：`src/fs/ext4_native/inode.rs`：4 处 `ensure_page()/pin_page().expect(...)` 改为 `.map_err(|_| Errno::EIO)?`（与 lwext4 后端同路径的处理一致）。

### #4 vfork 先阻塞后发布（C2）

- `src/kernel/syscall/task.rs` `do_clone` VFORK 分支：改为 `current::task().block_uninterruptible("vfork")` → `scheduler::push_task(child)` → `current::schedule()` + `take_wakeup_event()`。子进程在父进程切走前完成 exec/exit 时，唤醒方现在看到的是 `BlockedUninterruptible`（仍在 CPU 上）→ 置 `wake_pending` → `finish_switch` 转为重新入队，唤醒不再丢失。
- `src/kernel/task/tcb.rs` `wake_parent_waiting_vfork`：唤醒结果不再忽略，失败时带父/子 tid 打 `kwarn`。
- 顺带移除了因此变为死代码的 `current::block_uninterruptible` 助手。

### #5 block() 拒绝带 pending signal 入睡（C5）

- `src/kernel/task/tcb.rs` `TCB::block`：在发送方使用的同一把 TCB 状态锁下，若 `state.pending_signal.is_some()` 则不转入 Blocked——保持 Running、写入 `*wakeup_event = Some(Event::Signal)` 并返回 false。等待点随后照常注册并 `schedule()`：`finish_switch` 见 Running 即立刻重新入队，等待点的 `take_wakeup_event()` 取到 `Event::Signal`，走既有 EINTR 分支（该分支本就会取消注册，与真实跨核信号唤醒路径完全一致）。
- 注入的事件不会被覆盖：`wakeup()` 仅在 Blocked 态写事件槽；不经 schedule 直接退出的调用方走 `cancel_block()` 清槽。
- 不可中断路径（`block_uninterruptible`：睡眠锁/vfork/virtio）与 sigsuspend/sigtimedwait（`block_if_no_pending_signal`/`prepare_signal_wait` 专用路径）均未改动、行为不变。已逐一核对所有可中断等待点存在 `Event::Signal` 分支（pipe/msgpipe、futex/futex_waitv、nanosleep、epoll/poll/select、waitpid、stty、eventfd/timerfd、pty、fanotify、flock、msg/sem、tcp）。

### #6 exec 唤醒兄弟线程（H2 部分）+ poll/select 全量取消注册（H6）

- **H2**：`src/kernel/task/pcb/lifecycle.rs` `PCB::exec`：drain 兄弟线程与推入新 leader 并入同一临界区（消除"任务表为空"的窗口）；对每个被 drain 的兄弟线程（exec 发起线程除外）复刻 `PCB::exit` 的唤醒序列——`wake_parent_waiting_vfork()`、`resume_from_stopped()`/`resume_from_ptrace_stop()`、否则 `wakeup_task(_, Event::Signal)`。阻塞/停止的兄弟线程现在会被唤醒、经 `apply_pending_state_change` 观察到 dead 标记并释放 TCB/内核栈/地址空间引用。de_thread 式完整屏障（等兄弟线程全部 Exited 再提交新镜像）未实现，故标记[部分修复]。
- **H6**：`src/kernel/syscall/event.rs`：`do_poll` 与 `select` 唤醒后对**包括 waker 在内**的全部已注册文件调用 `wait_event_cancel()`。已逐一核实 poll 可达的所有 `wait_event_cancel` 实现（pipe、msgpipe、unixsocket、tty/stty、eventfd、timerfd、pty、epoll listener、inet、pidfd、fanotify、net port）均为按任务身份移除、不存在则 no-op 的幂等实现。**顺带发现并修复**：`src/arch/riscv/sbi_driver/char.rs` SBI 控制台驱动的 `wait_event_cancel` 原为 `unimplemented!()`（会 panic），改为与其从不注册的 `wait_event` 匹配的 no-op。

### #7 shm attach 三阶段重构（C7）+ virtio 唤醒链（H11）

- **C7**：`src/kernel/ipc/shm/manager.rs`：`attach` 拆为——阶段 1（持 SHM_MANAGER）：存在性/`deleted`(IPC_RMID)/权限检查，`ref_count += 1` 作为预约，克隆 `Arc<ShmFrames>` 后放锁（地址/标志校验与 perm 计算移到锁外，不触碰管理器状态）；阶段 2（不持 SHM_MANAGER）：`with_map_manager_mut` 选址、构建 `ShmArea`、完成映射——此时 `ShmArea::fork`/`Drop` 在 `map_manager.write()` 下回取 SHM_MANAGER 不再构成 ABBA；阶段 3（回锁）：成功则 `commit_attach` 更新 `lpid`/`atime` 并登记 `attach_map`，失败则 `on_area_drop(shmid)` 回滚预约（若段已 `deleted` 且计数归零则销毁）。预约计数在成功时转移给新 `ShmArea`（其 Drop 即配对递减），RMID-attach 竞争由预约计数保证段不会中途销毁，语义等同"attach 先于 RMID 发生"。已核实 `detach_shm_by_addr` 等其余路径无同型问题。
- **H11**：`src/driver/block/virtio.rs` `raw_read_blocks`/`raw_write_blocks`：`wake_next()` 移到 `complete_result` 错误传播**之前**，完成结果无论成败都接力唤醒下一个等待者。已核实该文件其余提前返回均发生在"拥有完成事件"之前。

### #8 信号路径去 SleepLock 化（C3、C4 根治）

- `src/kernel/task/pcb/mod.rs`：`PCB::tasks` 与 `PCB::child_wait` 由 SleepLock 改为 SpinLock（`child_wait` 必须一并转换：IRQ 上下文 `send_signal(SIGCONT/SIGSTOP)` 经 `resume_stopped_tasks → notify_continued → wake_waiting_tasks` 及 ptrace 停止通知可达）。
- `src/kernel/task/pcb/lifecycle.rs`：全部持锁区按"锁内快照/取出 → 锁外做睡眠操作"重构——`recycle` 锁内 `mem::take`、锁外 `manager::remove` 与 TCB drop（TCB 字段析构可睡眠）；`remove_task` 的移除结果延后到放锁后 drop；`exec` 的 drain+推新 leader 单临界区、锁外做 set_dead/唤醒；`exit` 以克隆快照迭代，`close_all()`（fd 表 SleepLock）移出 `tasks` 锁外。
- `src/kernel/ipc/signal/handle.rs`：`stop_tasks_from_signal` 改为克隆快照迭代（`request_ptrace_stop` 可能通知 tracer 的 PCB，不得嵌套在本进程 `tasks` 锁内——既防跨 PCB ABBA 也防 IRQ 路径嵌套）。
- `src/kernel/task/manager.rs`：`with_initpcb` 先克隆 init 的 `Arc<PCB>` 并释放全局 TCBS 锁再执行闭包，消除 `child_wait` 转 SpinLock 后 exit 重新挂靠孤儿（TCBS → child_wait）与 `wait` 路径（child_wait → TCBS）之间的新 ABBA。
- `src/driver/char/serial/stty.rs` `handle_interrupt`：持 `serial` 锁仅抽干 UART FIFO 到本地缓冲，放锁后再做 TTY 输入处理与发信号（信号代码内的 kwarn 若走控制台路径会再取 `serial`，此前存在同核自死锁可能）；"字节先入 tty、再唤醒 waiters/epoll"的顺序保持不变。M2 的信号目标/SIGQUIT 语义原样保留，另行处理。
- 全量审计结论：16 处 `tasks.lock()`、10 处 `child_wait.lock()` 调用点逐一分类核查（保留在锁内的均验证为 spin-only：TCB 状态锁、调度器唤醒/入队、`signal.actions`/`signal.pending`、TimerTable、timerfd 等）；IRQ 可达的全部定时器到期回调（posix timer、timerfd、itimer/alarm、任务唤醒型）验证为 spin-only；`send_signal` 可达集合内已无任何睡眠锁。

### 验证与遗留

- 每一波修复后均通过 `make check`（0 错误）；最终 `make kernel` 完整构建并成功链接出 `build/riscv64/vmkernelx`。
- 竞态修复已通过真实负载回归：8 核 QEMU 下完整跑通 buildstorm 测评（约 13 分钟持续的 8 核 cargo 编译，含 fork/exec 风暴、缺页风暴、海量文件 I/O），无 panic、无死锁、无新增内核警告。
- 建议进一步定向压测：vfork/execve 密集、多线程 + 信号（kill 风暴）、`open;unlink;write` 模式、shmat 与 fork 并发、poll/select 于 socketpair、Ctrl-C。
- 仍未修复的高优先级项（建议后续按序处理）：C6（pipe 状态锁改造）、H1（pin-aware fork）、H3/H4（dentry 目录锁与 umount 互斥）、H7（定时器 generation）、H8（sigmask 交换原子性）、H9（凭证单锁化）、M14（等待点 spurious-wakeup 纪律）；性能架构项 P3 剩余（per-hart 队列）、P5（futex 分桶）、P6 剩余（sb 锁内设备 I/O、readahead）。

---

## 七、性能优化记录（2026-07-26）

四波优化全部完成并经真实测评负载验证。**实测效果：buildstorm 8 核编译测评 `elapsed_s` 从 1126.30s 降至 780.84s（-30.7%）**，从超出 800s 零分线降入得分区间；guest 内 cargo 计时段从 18m08s 降至 12m29s。工具链/minibuild/编译成功各阶段全部通过，全程无内核异常。

### #A TLB/satp 路径（P1 + P2）

- `src/arch/riscv/pagetable/pagetable.rs`：新增 `stale_cpu_mask` 延迟冲刷机制（全部在既有页表自旋锁下维护）——失效操作对未同步 fence 的 hart 置 stale 位，`activate_cpu` 发现自己欠账时补一次全量本地 `sfence.vma`。这替代了原先"每次返回用户态无条件双 sfence"的正确性来源。
- `clib/src/arch/riscv/trap/usertrap.S` 与 `swtich.S`：satp 与当前值比较，相同则完全跳过 csrw + fence；不同才 csrw + 1 次 sfence（原为无条件 2 次）。
- `mmap_raw`（invalid→valid 首次装页）与 A/D 位升级：不再发远程 shootdown、不置 stale，只做本地单页 `sfence.vma`——其他 hart 无陈旧有效项，缺页自愈。为此核实并加固了 spurious-fault 容忍：`traphandle.rs` 增加故障后本地单页 fence 兜底；顺带修复 shm 并发缺页会触发 `mmap_raw` debug assert 的隐藏 bug（改走 `mmap_replace`）。
- 其余失效路径（unmap/mprotect/替换）改用新增 `flush_tlb_range`：≤32 页逐页 fence + 范围化 SBI RFENCE（新增 `remote_sfence_vma_range`），超限回退全量。
- 关键边界：新建/复用页表根帧时 `stale_cpu_mask` 初始化为全 1（防止 exec 复用根帧得到位相同的 satp 而跳过冲刷）；swap 回收路径保持同步远程 fence（不能依赖惰性自愈）+ 置全员 stale 兜底；loongarch 提供等价语义的桩，接口不破坏。
- 验证：objdump 确认两处汇编生成预期的 `csrr/beq/csrw/sfence.vma` 序列；`activate_cpu` 在所有返回用户态路径（正常 trap 出口、sigreturn、新任务首次进入）都先于 satp 恢复执行。

### #B 文件系统刷盘瘦身（P6 部分 + A10）

- `src/fs/vfs/dentry.rs` `remove_child`：移除 unlink 路径的 `sync()`——lwext4 未启用日志，原全量刷盘不提供崩溃原子性，纯为浪费；打开中的 fd 由 C9 的 orphan 延迟释放保障。
- `src/fs/ext4/superblock.rs`：`flush()` 拆分为 `flush_device()`（bcache 刷盘 + sb 写 + 设备 barrier，保留 inode-ref 缓存）与完整 flush（额外 drop ref 缓存）。
- `src/fs/ext4/inode.rs`：inode 淘汰/drop 的 `sync()` 只回写自身脏页 + put 自身 inode-ref（不刷 bcache、无 barrier）；新增 `fsync()`（脏页 + ref + `flush_device()`）接入 `VfsInodeOps`/`RandomAccessFile::fsync`；`syncfs(2)` 从静默 no-op 修正为 `vfs::sync_all()`。全量刷盘入口收敛为：sync(2)/syncfs(2)/fsync(2)/remount-ro/umount。
- **A10 一并修复**：`lookup()` 不再每个路径分量写父目录 atime（原先把所有读放大成写），`readlink()` 同。
- `size()`：常规文件改读原子 `cached_size`（逐点审计确认 new/truncate/fallocate/写扩展全部维护到位、无缺口），`fstat`/`lseek SEEK_END`/O_APPEND 均免全局超级块锁。

### #C 时钟、记账与 CoW 分配（P7 + P4 + P8 部分）

- `src/driver/chosen/kclock.rs` 重写：首次读取 RTC 后记录 `REALTIME_OFFSET_NS`（AtomicU64，CAS 首写者胜出），此后 REALTIME = 偏移 + monotonic CSR——零锁零 MMIO；`settimeofday`/`clock_settime` 更新偏移。gettimeofday/clock_gettime/timerfd/文件时间戳全部受益。
- `src/kernel/event/posix_timer.rs` + `tcb.rs`：`TimerTable` 增加 `task_cpu_timers` 原子计数（仅 CLOCK_PROCESS/THREAD_CPUTIME_ID 计入），`check_cpu_timers` 一次 Relaxed load 即短路——消除每次 syscall 进出的双重锁 + `collect()` 分配；已核实 wall-clock 定时器走全局堆路径、与此无关。
- `src/kernel/trap.rs` + `pcb/usage.rs`：syscall 进出不再取全进程共享的 `tasks_time_usage` 锁；时间累计在 per-TCB `time_counter`（新增 folded 字段），每次上下文切换折叠进 PCB 一次；读者（getrusage/times/procfs）= PCB 累计 + 各活跃线程未折叠残量 + 在途运行区间（比旧实现更精确）。已核查锁序无反向边。
- CoW 三处（`anonymous/private.rs`、`userstack.rs`、`filemap/private.rs` 的 `copy_on_write_page`）：`alloc_with_shrink_zeroed` → `alloc_with_shrink`——随后的 `copy_from_slice` 恰为整页覆盖（`slice()` 定长 PGSIZE），且发生在装入任何 PTE 之前；新匿名页/共享页等必须清零的分配点全部保留清零。

### #D 调度 IPI 与 tick 跳过（P3 部分）

- `src/arch/riscv/sbi_driver/sbi.rs`：新增 `send_ipi(cpu_mask)`（SBI IPI 扩展 0x735049），经 `arch_export` 导出；loongarch no-op 桩（保持其现状：靠 tick 唤醒）。
- `src/kernel/scheduler/scheduler.rs`：新增 `IDLE_HARTS` 原子位图 + `ready_count` 镜像；所有入队路径经 `push_task` 后 `kick_idle_hart()`——最多 IPI 一个空闲 hart（最低位，排除自身）。丢唤醒窗口以 SeqCst Dekker 序关闭：空闲侧"置位 → 复查队列（非空则清位重试）→ wfi"，入队侧"写 ready_count → 读位图"；即使假设性漏掉也被 10ms tick 兜底。
- `src/arch/riscv/task/traphandle.rs`：软件中断处理从仅打日志改为 `sip::clear_ssoft()`；核实 SSIE 此前从未使能，新增 `enable_software_interrupt()` 在每个 hart 的启动路径调用。
- `src/kernel/trap.rs` `timer_interrupt`：就绪队列空时跳过 `current::schedule()` 全套往返（`timer::interrupt()` 回调仍每 tick 无条件执行；队列空时本就无可切换对象，不会饿死任何任务）。

### 实测数据（8 核 16G QEMU，buildstorm-glibc 测评）

| 指标 | 优化前 | 优化后 | 变化 |
|------|--------|--------|------|
| BUILDSTORM_COMPILE elapsed_s | 1126.30 | 780.84 | **-30.7%** |
| guest 内 cargo 计时段 | 18m08s | 12m29s | -31.2% |
| axbuild 报告的构建耗时 | 1104.90s | 763.67s | -30.9% |
| 时间分（120 满分，基线 400s） | 0 | ≈5.7 | 进入得分区间 |
| TOOLCHAIN / MINIBUILD / COMPILE ok | 全过 | 全过 | 无回归 |

注：测评文档标称 8c/8G，实际测评环境为 8c/**16G**，与本次实测配置一致。swap 子系统整场 0 回收（LRU 仅追踪），内存非瓶颈。继续压缩耗时的下一批候选：P6 剩余（sb 锁内设备 I/O 移出、readahead——编译负载读大量 .rs/.rlib）、P5（futex 分桶——rustc 多线程内部同步）、P3 剩余（per-hart 队列）、P8 剩余（堆/页帧分配器 per-CPU 缓存）。
