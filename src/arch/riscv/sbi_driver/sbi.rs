type SBIRet = Result<usize, isize>;

#[repr(usize)]
enum SBIExtension {
    Hsm = 0x48534d,
    #[cfg(not(feature = "no-smp"))]
    Ipi = 0x735049,
    #[cfg(not(feature = "no-smp"))]
    Rfence = 0x52464e43,
}

#[repr(usize)]
enum HsmFunction {
    HartStart = 0,
}

#[cfg(not(feature = "no-smp"))]
#[repr(usize)]
enum RfenceFunction {
    RemoteSfenceVma = 1,
}

#[cfg(not(feature = "no-smp"))]
#[repr(usize)]
enum IpiFunction {
    SendIpi = 0,
}

fn sbi_call(
    fid: usize,
    eid: usize,
    arg0: usize,
    arg1: usize,
    arg2: usize,
    arg3: usize,
    arg4: usize,
    arg5: usize,
) -> SBIRet {
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
    if error == 0 { Ok(value) } else { Err(error) }
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
    sbi_call(
        HsmFunction::HartStart as usize,
        SBIExtension::Hsm as usize,
        hartid,
        start_addr,
        opaque,
        0,
        0,
        0,
    )
}

#[cfg(not(feature = "no-smp"))]
pub fn send_ipi(cpu_mask: usize) -> SBIRet {
    debug_assert!(cpu_mask != 0);
    sbi_call(
        IpiFunction::SendIpi as usize,
        SBIExtension::Ipi as usize,
        cpu_mask,
        0,
        0,
        0,
        0,
        0,
    )
}

#[cfg(not(feature = "no-smp"))]
pub fn remote_sfence_vma_all() -> SBIRet {
    // A hart-mask base of -1 selects every SBI-available hart. A zero start
    // address and zero size request invalidation of the entire address space.
    sbi_call(
        RfenceFunction::RemoteSfenceVma as usize,
        SBIExtension::Rfence as usize,
        0,
        usize::MAX,
        0,
        0,
        0,
        0,
    )
}

#[cfg(not(feature = "no-smp"))]
pub fn remote_sfence_vma(cpu_mask: usize) -> SBIRet {
    debug_assert!(cpu_mask != 0);
    sbi_call(
        RfenceFunction::RemoteSfenceVma as usize,
        SBIExtension::Rfence as usize,
        cpu_mask,
        0,
        0,
        0,
        0,
        0,
    )
}
