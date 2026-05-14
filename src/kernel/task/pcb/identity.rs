use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::fs::Dentry;
use crate::kernel::ipc::{PendingSignalQueue, SignalActionTable};
use crate::kernel::uapi::Uid;
use crate::klib::SpinLock;

use super::*;

impl PCB {
    pub fn pid(&self) -> Tid {
        self.pid
    }

    pub fn wait_parent_tid(&self) -> Tid {
        *self.wait_parent_tid.lock()
    }

    pub(super) fn set_wait_parent_tid(&self, tid: Tid) {
        *self.wait_parent_tid.lock() = tid;
    }

    pub fn pgid(&self) -> Pid {
        *self.pgid.lock()
    }

    pub fn set_pgid(&self, pgid: Pid) {
        *self.pgid.lock() = pgid;
    }

    pub fn sid(&self) -> Pid {
        *self.sid.lock()
    }

    pub fn set_sid(&self, sid: Pid) {
        *self.sid.lock() = sid;
    }

    pub fn is_session_leader(&self) -> bool {
        self.sid() == self.pid()
    }

    pub fn has_execed(&self) -> bool {
        *self.execed.lock()
    }

    pub fn uid(&self) -> Uid {
        *self.uid.lock()
    }

    pub fn set_uid(&self, uid: Uid) {
        *self.uid.lock() = uid;
    }

    pub fn euid(&self) -> Uid {
        *self.euid.lock()
    }

    pub fn set_euid(&self, euid: Uid) {
        *self.euid.lock() = euid;
        self.set_fsuid(euid);
    }

    pub fn suid(&self) -> Uid {
        *self.suid.lock()
    }

    pub fn set_suid(&self, suid: Uid) {
        *self.suid.lock() = suid;
    }

    pub fn fsuid(&self) -> Uid {
        *self.fsuid.lock()
    }

    pub fn set_fsuid(&self, fsuid: Uid) {
        *self.fsuid.lock() = fsuid;
    }

    pub fn gid(&self) -> Uid {
        *self.gid.lock()
    }

    pub fn set_gid(&self, gid: Uid) {
        *self.gid.lock() = gid;
    }

    pub fn egid(&self) -> Uid {
        *self.egid.lock()
    }

    pub fn set_egid(&self, egid: Uid) {
        *self.egid.lock() = egid;
        self.set_fsgid(egid);
    }

    pub fn sgid(&self) -> Uid {
        *self.sgid.lock()
    }

    pub fn set_sgid(&self, sgid: Uid) {
        *self.sgid.lock() = sgid;
    }

    pub fn fsgid(&self) -> Uid {
        *self.fsgid.lock()
    }

    pub fn set_fsgid(&self, fsgid: Uid) {
        *self.fsgid.lock() = fsgid;
    }

    pub fn supplementary_gids(&self) -> Vec<Uid> {
        self.supplementary_gids.lock().clone()
    }

    pub fn set_supplementary_gids(&self, gids: Vec<Uid>) {
        *self.supplementary_gids.lock() = gids;
    }

    pub fn nice(&self) -> isize {
        *self.nice.lock()
    }

    pub fn set_nice(&self, nice: isize) {
        *self.nice.lock() = nice;
    }

    pub fn capabilities(&self) -> ProcessCapabilities {
        *self.capabilities.lock()
    }

    pub fn set_capabilities(&self, capabilities: ProcessCapabilities) {
        *self.capabilities.lock() = capabilities;
    }

    pub fn exec_path(&self) -> String {
        self.exec_path.lock().clone()
    }

    pub fn uts(&self) -> &UtsNamespace {
        &self.uts
    }

    pub fn is_exited(&self) -> bool {
        matches!(*self.state.lock(), State::Exited(_))
    }

    pub fn cwd(&self) -> Arc<Dentry> {
        self.cwd.lock().clone()
    }

    pub fn set_cwd(&self, dentry: &Arc<Dentry>) {
        *self.cwd.lock() = dentry.clone();
    }

    pub fn root(&self) -> Arc<Dentry> {
        self.root.lock().clone()
    }

    pub fn set_root(&self, dentry: &Arc<Dentry>) {
        *self.root.lock() = dentry.clone();
    }

    pub fn umask(&self) -> u16 {
        *self.umask.lock()
    }

    pub fn set_umask(&self, mask: u16) {
        *self.umask.lock() = mask & 0o777;
    }

    pub fn file_size_limit(&self) -> (usize, usize) {
        *self.file_size_limit.lock()
    }

    pub fn set_file_size_limit(&self, cur: usize, max: usize) {
        *self.file_size_limit.lock() = (cur, max);
    }

    pub fn signal_actions(&self) -> &SpinLock<SignalActionTable> {
        &self.signal.actions
    }

    pub fn pending_signals(&self) -> &SpinLock<PendingSignalQueue> {
        &self.signal.pending
    }
}
