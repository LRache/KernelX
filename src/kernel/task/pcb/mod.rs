mod capability;
mod identity;
mod lifecycle;
mod status;
mod usage;
mod wait;

pub use capability::{CapabilitySet, ProcessCapabilities};
use status::{ChildWaitStatus, State};
pub use status::{ExitStatus, ITimer, WaitStatus};
pub use wait::{ChildWaitOptions, WaitResult};

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::time::Duration;

use crate::fs::{Dentry, Inode};
use crate::kernel::event::{Event, TimerTable, WaitQueue};
use crate::kernel::ipc::{PendingSignalQueue, SignalActionTable, SignalNum};
use crate::kernel::scheduler::Task;
use crate::kernel::scheduler::tid::Tid;
use crate::kernel::uapi::Uid;
use crate::klib::SpinLock;

use super::UtsNamespace;
use super::tcb::TCB;

pub type Pid = Tid;

struct Signal {
    actions: SpinLock<SignalActionTable>,
    pending: SpinLock<PendingSignalQueue>,
}

struct ChildWaitState {
    children: Vec<Arc<PCB>>,
    waiters: Vec<(Arc<dyn Task>, Option<Tid>)>,
}

impl ChildWaitState {
    fn new() -> Self {
        Self {
            children: Vec::new(),
            waiters: Vec::new(),
        }
    }
}

pub struct PCB {
    pid: Tid,
    pub parent: SpinLock<Option<Arc<PCB>>>,
    /// Parent thread that may wait this child when __WNOTHREAD is set.
    wait_parent_tid: SpinLock<Tid>,
    state: SpinLock<State>,
    child_wait_status: SpinLock<Option<ChildWaitStatus>>,
    exec_path: SpinLock<String>,
    exec_inode: SpinLock<Option<Arc<Inode>>>,

    // Signal delivery may lock `tasks` from interrupt context (tty, timer
    // expiry), so it must stay a spin lock and its holders must not sleep.
    pub tasks: SpinLock<Vec<Arc<TCB>>>,
    root: SpinLock<Arc<Dentry>>,
    cwd: SpinLock<Arc<Dentry>>,
    umask: SpinLock<u16>,
    file_size_limit: SpinLock<(usize, usize)>,
    // Reached from interrupt-context signal delivery via wake_waiting_tasks
    // (SIGCONT/SIGCHLD notification), so it must stay a spin lock as well.
    child_wait: SpinLock<ChildWaitState>,
    pidfd_waiters: SpinLock<WaitQueue<Event>>,
    uts: UtsNamespace,

    signal: Signal,

    pub itimers: SpinLock<[Option<ITimer>; 3]>,
    pub timers: TimerTable,

    // TODO: 减少鉴权时候的数据拷贝。
    uid: SpinLock<Uid>,
    euid: SpinLock<Uid>,
    suid: SpinLock<Uid>,
    fsuid: SpinLock<Uid>,
    gid: SpinLock<Uid>,
    egid: SpinLock<Uid>,
    sgid: SpinLock<Uid>,
    fsgid: SpinLock<Uid>,
    supplementary_gids: SpinLock<Vec<Uid>>,
    nice: SpinLock<isize>,
    capabilities: SpinLock<ProcessCapabilities>,

    pgid: SpinLock<Pid>,
    sid: SpinLock<Pid>,
    execed: SpinLock<bool>,
    exit_signal: SignalNum,

    /// Cumulative CPU time of this process' own threads.
    tasks_time_usage: SpinLock<(Duration, Duration)>,
    /// CPU time snapshot taken at exit (own threads). Preserved after recycle clears tasks.
    tasks_time_usage_capture: SpinLock<(Duration, Duration)>,
    /// Cumulative CPU time of all waited-for (reaped) children, including their descendants.
    waited_children_time_usage: SpinLock<(Duration, Duration)>,
}

unsafe impl Send for PCB {}
unsafe impl Sync for PCB {}
