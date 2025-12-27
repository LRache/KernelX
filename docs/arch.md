# 体系结构层

体系结构层主要提供了与硬件架构相关的抽象和接口，涵盖了以下几个方面。

```rust
pub trait ArchTrait {
    /// 初始化体系结构相关的功能
    fn init();
    /// 内核基本数据结构初始化完成后，启动所有核心
    fn setup_all_cores(current_core: usize);
    
    /* ----- Per-CPU Data ----- */
    fn set_percpu_data(data: usize);
    fn get_percpu_data() -> usize;

    /* ----- Context Switching ----- */
    /// 切换内核态上下文
    fn kernel_switch(from: *mut KernelContext, to: *mut KernelContext);
    /// 获取用户态程序 pc
    fn get_user_pc() -> usize;
    /// 强行返回用户态
    fn return_to_user() -> !;
    
    /* ----- Interrupt ------ */
    /// 等待中断，用于调度器空闲时调用
    fn wait_for_interrupt();
    /// 启用/禁用中断
    fn enable_interrupt();
    fn disable_interrupt();
    fn enable_timer_interrupt();
    fn enable_device_interrupt();
    fn enable_device_interrupt_irq(irq: u32);

    /// 获取内核栈顶地址，用于监视内核资源使用
    fn get_kernel_stack_top() -> usize;

    /// 进行地址转换
    fn kaddr_to_paddr(kaddr: usize) -> usize;
    fn paddr_to_kaddr(paddr: usize) -> usize;
    /// 扫描设备
    fn scan_device();
    /// 内核地址映射与取消映射
    fn map_kernel_addr(kstart: usize, pstart: usize, size: usize, perm: MapPerm);
    unsafe fn unmap_kernel_addr(kstart: usize, size: usize);

    /// 获取系统运行时间
    fn uptime() -> Duration;
    fn get_time_us() -> u64;

    /// 设置下一个定时事件，用于定时器中断
    fn set_next_time_event_us(interval: u64);
}
```

这些函数都可以通过 `arch::function_name` 的方式调用，例如 `arch::init()`。`PageTable` 相关的接口已经在内存管理章节中介绍。

内核态的上下文切换是以函数形式调用的，所以 `KernelContext` 并不需要保存所有寄存器，只需要保存调用约定中需要保存的寄存器即可。`Arch::kernel_switch` 函数会保存当前内核态上下文到 `from` 指针指向的结构中，然后加载 `to` 指针指向的结构中的上下文，最后返回到新的内核态上下文继续执行。

内核提供了处理相应 trap 的接口，在发生了 trap 的时候，体系结构层应该调用这些接口来处理相应的事件：

```rust
// src/kernel/trap.rs
/// 在进入 Trap 处理前部调用
pub fn trap_enter();
/// 在返回用户态之前调用
pub fn trap_return();
/// 处理定时器中断
pub fn timer_interrupt();
/// 处理系统调用，返回值是系统调用的返回值
pub fn syscall(num: usize, args: &syscall::Args) -> usize;
/// 处理内存访问错误
pub fn memory_fault(addr: usize, access_type: MemAccessType);
/// 处理非法指令异常
pub fn illegal_inst();
/// 处理对齐错误异常
pub fn memory_misaligned();
/// 处理外部中断，`irq` 是设备的中断号
pub fn external_interrupt(irq: u32);
```

在 `trap_enter` 和 `trap_return` 中，内核会记录时间点，用于统计任务的用户态和内核态时间。`trap_return` 会在返回用户态之前，检查当前线程是否有待处理的信号，如果有则进行信号处理。`timer_interrupt` 会设置一个新的时间中断，同时检查计时器中是否有需要唤醒的任务。`syscall` 会根据系统调用号调用相应的系统调用处理函数。`memory_fault` 会处理缺页异常和访问权限异常。`external_interrupt` 会根据中断号调用相应的设备中断处理函数。