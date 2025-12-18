use core::time::Duration;

use alloc::sync::Arc;
use spin::Mutex;

use crate::kernel::event::{Event, timer};
use crate::kernel::ipc::SignalActionTable;
use crate::kernel::mm::AddrSpace;
use crate::kernel::scheduler::task::Task;
use crate::kernel::task::{PCB, TCB};
use crate::kernel::task::fdtable::FDTable;
use crate::kernel::scheduler::Processor;
use crate::kernel::uapi::Uid;
use crate::arch;
use crate::fs::Dentry;
use crate::klib::SpinLock;

use super::Tid;

pub fn processor() -> &'static mut Processor {
    let p = arch::get_percpu_data() as *mut Processor;
    
    debug_assert!(!p.is_null());
    
    unsafe { &mut *p }
}

pub fn set(p: &Processor) {
    arch::set_percpu_data(p as *const Processor as usize);
}

pub fn has_task() -> bool {
    arch::get_percpu_data() != 0 && processor().has_task()
}

pub fn hart_id() -> usize {
    processor().hart_id()
}

pub fn task() -> &'static Arc<dyn Task> {
    let processor = processor();
    &processor.task()
}

pub fn tcb() -> &'static TCB {
    let processor = processor();
    processor.tcb()
}

pub fn tid() -> Tid {
    if !has_task() {
        -1
    } else {
        task().tid()
    }
}

pub fn pid() -> Tid {
    if !has_task() {
        -1
    } else {
        pcb().get_pid()
    }
}

pub fn uid() -> Uid {
    0
}

pub fn pcb() -> &'static Arc<PCB> {
    tcb().get_parent()
}

pub fn signal_actions() -> &'static Mutex<SignalActionTable> {
    let pcb = pcb();
    pcb.signal_actions()
}

pub fn addrspace() -> &'static Arc<AddrSpace> {
    let tcb = tcb();
    tcb.get_addrspace()
}

pub fn fdtable() -> &'static SpinLock<FDTable> {
    let tcb = tcb();
    tcb.fdtable()
}

pub fn with_cwd<F, R>(f: F) -> R 
where F: FnOnce(&Arc<Dentry>) -> R {
    let pcb = pcb();
    pcb.with_cwd(f)
}

pub mod copy_to_user {
    use crate::kernel::errno::SysResult;
    use super::addrspace;

    pub fn buffer(uaddr: usize, buf: &[u8]) -> SysResult<()> {
        addrspace().copy_to_user_buffer(uaddr, buf)
    }

    pub fn object<T: Copy>(uaddr: usize, value: T) -> SysResult<()> {
        addrspace().copy_to_user(uaddr, value)
    }

    pub fn slice<T: Copy>(uaddr: usize, slice: &[T]) -> SysResult<()> {
        addrspace().copy_to_user_slice(uaddr, slice)
    }

    pub fn string(uaddr: usize, s: &str, max_size: usize) -> SysResult<usize> {
        let bytes = s.as_bytes();
        let len = core::cmp::min(bytes.len(), max_size - 1);
        addrspace().copy_to_user_buffer(uaddr, &bytes[..len])?;
        addrspace().copy_to_user_buffer(uaddr + len, &[0u8])?;
        Ok(len)
    }
}

pub mod copy_from_user {
    use alloc::string::String;
    use crate::kernel::errno::SysResult;
    use super::addrspace;

    pub fn buffer(uaddr: usize, buf: &mut [u8]) -> SysResult<()> {
        addrspace().copy_from_user_buffer(uaddr, buf)
    }

    pub fn object<T: Copy>(uaddr: usize) -> SysResult<T> {
        addrspace().copy_from_user::<T>(uaddr)
    }

    pub fn string(uaddr: usize) -> SysResult<String> {
        addrspace().get_user_string(uaddr)
    }

    pub fn slice<T: Copy>(uaddr: usize, slice: &mut [T]) -> SysResult<()> {
        addrspace().copy_from_user_slice(uaddr, slice)
    }
}

pub fn schedule() {
    processor().schedule()
}

pub fn block(reason: &'static str) -> Event {
    task().block(reason);
    schedule();
    task().take_wakeup_event().unwrap()
}

// pub fn block_sigmask(reason: &'static str, mask: SignalSet) -> Event {
//     let old_mask = tcb().swap_signal_mask(mask);
//     task().block(reason);
//     schedule();
//     tcb().set_signal_mask(old_mask);
//     task().take_wakeup_event().unwrap()
// }

pub fn block_uninterruptible(reason: &'static str) -> Event {
    task().block_uninterruptible(reason);
    schedule();
    task().take_wakeup_event().unwrap()
}

pub fn sleep(durations: Duration) -> Event {
    timer::add_timer(task().clone(), durations);
    task().block("sleep");
    schedule();
    task().take_wakeup_event().unwrap()
}

pub fn umask() -> u32 {
    pcb().umask() as u32
}
