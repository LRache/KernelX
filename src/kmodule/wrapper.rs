use core::{slice, str};

use crate::kernel::errno::Errno;
use crate::kernel::scheduler::current;
use crate::klib::klog::{COLOR_BLUE, COLOR_BOLD, COLOR_RESET};
use crate::kmodule::exports::kmodule_export;

const MAX_KMODULE_LOG_LEN: usize = 1024;
const MAX_KMODULE_FILE_LEN: usize = 256;

// SAFETY: This symbol is part of the stable C ABI exposed to kernel modules.
#[kmodule_export]
#[unsafe(no_mangle)]
pub extern "C" fn kmodule_log_info(file: *const u8, line: u32, column: u32, msg: *const u8, len: usize) -> isize {
    if msg.is_null() {
        return -(Errno::EINVAL as isize);
    }

    let len = len.min(MAX_KMODULE_LOG_LEN);
    // SAFETY: The kmodule logging ABI requires msg to point to len readable
    // bytes; len is capped here before constructing the slice.
    let msg = unsafe { slice::from_raw_parts(msg, len) };
    let msg = str::from_utf8(msg).unwrap_or("<invalid utf8 kmodule log>");
    // SAFETY: cstr bounds the scan to MAX_KMODULE_FILE_LEN and accepts a null
    // pointer by returning None.
    let file = unsafe { cstr(file, MAX_KMODULE_FILE_LEN) }.unwrap_or("<unknown>");

    crate::klogln!(
        "{}{}[{}]{} {} (tid={}) @ {}:{}:{}{}",
        COLOR_BOLD,
        COLOR_BLUE,
        "INFO",
        COLOR_RESET,
        msg,
        current::tid(),
        file,
        line,
        column,
        COLOR_RESET
    );

    len as isize
}

/// # Safety
///
/// ptr must be either null or point to a readable C string whose terminator
/// appears within max_len bytes.
unsafe fn cstr(ptr: *const u8, max_len: usize) -> Option<&'static str> {
    if ptr.is_null() {
        return None;
    }

    let mut len = 0;
    while len < max_len {
        // SAFETY: The caller guarantees ptr is readable until the first NUL byte
        // within max_len bytes; len is kept below max_len by the loop condition.
        if unsafe { *ptr.add(len) } == 0 {
            break;
        }
        len += 1;
    }

    // SAFETY: The scan above found a byte length bounded by max_len, and the
    // caller guarantees that range is readable.
    let bytes = unsafe { slice::from_raw_parts(ptr, len) };
    str::from_utf8(bytes).ok()
}
