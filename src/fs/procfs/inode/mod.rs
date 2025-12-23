mod root;
mod task;
mod taskself;

pub use root::{RootInode, MountsInode};
pub use task::{TaskDirInode, TaskMapsInode, TaskExeInode};
pub use taskself::TaskDirSelfInode;

use alloc::string::String;

use crate::kernel::errno::SysResult;
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

fn read_iter_text<T, F>(buf: &mut [u8], offset: usize, iterator: T, f: F) -> SysResult<usize>
where T: Iterator, F: Fn(T::Item) -> SysResult<String>  {
    let mut pos = 0usize;
    let mut copied = 0usize;

    for item in iterator {
        let line = f(item)?;
        let line_len = line.bytes().len();
        
        if pos + line_len <= offset {
            pos += line_len;
            continue;
        }

        if copied >= buf.len() {
            break;
        }

        let line_bytes = line.as_bytes();
        let start_in_line = offset.saturating_sub(pos);
        let left_in_line = line_len.saturating_sub(start_in_line);
        let to_copy = core::cmp::min(left_in_line, buf.len() - copied);
        
        if to_copy == 0 {
            pos += line_len;
            continue;
        }

        buf[copied..copied + to_copy]
            .copy_from_slice(&line_bytes[start_in_line..start_in_line + to_copy]);
        copied += to_copy;
        pos += line_len;

        if copied == buf.len() {
            break;
        }
    }

    Ok(copied)
}
