struct SBIRet {
    _error: usize,
    _value: usize,
}

fn sbi_call(fid: usize, eid: usize, arg0: usize, arg1: usize, arg2: usize, arg3: usize, arg4: usize, arg5: usize) -> SBIRet {
    let mut error;
    let mut value;
    
    unsafe {
        core::arch::asm!(
            "ecall",
            inlateout("a0") arg0 => error,
            inlateout("a1") arg1 => value,
            in("a2") arg2,
            in("a3") arg3,
            in("a4") arg4,
            in("a5") arg5,
            in("a6") fid,
            in("a7") eid,
            options(nostack, preserves_flags)
        );
    }
    SBIRet { 
        _error: error,
        _value: value,
    }
}

pub fn shutdown() -> ! {
    sbi_call(0x0, 0x8, 0, 0, 0, 0, 0, 0);
    
    loop {
        unsafe {
            core::arch::asm!("wfi");
        }
    }
}

pub fn putchar(c: u8) -> () {
    sbi_call(0x0, 0x1, c as usize, 0, 0, 0, 0, 0);
}

pub fn set_timer(time: u64) {
    sbi_call(0x0, 0x0, time as usize, (time >> 32) as usize, 0, 0, 0, 0);
}
