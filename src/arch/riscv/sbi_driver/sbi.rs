type SBIRet = Result<usize, isize>;

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
    if error == 0 {
        Ok(value)
    } else {
        Err(error)
    }
}

pub fn shutdown() -> ! {
    let _ = sbi_call(0x0, 0x8, 0, 0, 0, 0, 0, 0);
    
    loop {
        unsafe {
            core::arch::asm!("wfi");
        }
    }
}

pub fn putchar(c: u8) {
    let _ = sbi_call(0x0, 0x1, c as usize, 0, 0, 0, 0, 0);
}

pub fn set_timer(time: u64) {
    let _ = sbi_call(0x0, 0x0, time as usize, (time >> 32) as usize, 0, 0, 0, 0);
}

pub fn hart_start(hartid: usize, start_addr: usize, opaque: usize) -> SBIRet {
    sbi_call(0x0, 0x2, hartid, start_addr, opaque, 0, 0, 0)
}
