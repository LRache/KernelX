mod root;
mod task;
mod taskself;

pub use root::RootInode;
pub use task::{TaskDirInode, TaskMapsInode, TaskExeInode};
pub use taskself::TaskDirSelfInode;

use crate::kernel::task::TCB;
use crate::kernel::uapi::FileStat;

fn fill_kstat_common(kstat: &mut FileStat, tcb: &TCB) {
    kstat.st_uid = 0;
    kstat.st_gid = 0;
    kstat.st_size = 0;
    let time = tcb.create_time();
    kstat.st_atime_sec = time.as_secs() as i64;
    kstat.st_atime_nsec = (time.subsec_nanos()) as i64;
    kstat.st_mtime_sec = time.as_secs() as i64;
    kstat.st_mtime_nsec = (time.subsec_nanos()) as i64;
    kstat.st_ctime_sec = time.as_secs() as i64;
    kstat.st_ctime_nsec = (time.subsec_nanos()) as i64;
}
