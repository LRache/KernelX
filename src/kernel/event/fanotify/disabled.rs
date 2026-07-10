use alloc::sync::Arc;

use crate::fs::Dentry;
use crate::fs::file::FileOps;
use crate::kernel::errno::SysResult;

use super::types::FanotifyEventMask;

pub fn wait_fanotify_permission(_file: &Arc<dyn FileOps>, _mask: FanotifyEventMask) -> SysResult<()> {
    Ok(())
}

pub fn wait_fanotify_open_exec_permission(_file: &Arc<dyn FileOps>) -> SysResult<()> {
    Ok(())
}

pub fn notify_fanotify(_file: &Arc<dyn FileOps>, _mask: FanotifyEventMask) {}

pub fn notify_fanotify_dentry(_dentry: &Arc<Dentry>, _mask: FanotifyEventMask) {}
