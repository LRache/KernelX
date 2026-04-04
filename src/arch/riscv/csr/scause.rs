use num_enum::TryFromPrimitive;

#[derive(Debug, Clone, Copy, PartialEq, Eq, TryFromPrimitive)]
#[repr(usize)]
pub enum Trap {
    InstAddrMisaligned = 0,
    InstAccessFault = 1,
    IllegalInst = 2,
    Breakpoint = 3,
    LoadAddrMisaligned = 4,
    LoadAccessFault = 5,
    StoreAddrMisaligned = 6,
    StoreAccessFault = 7,
    EcallU = 8,
    EcallS = 9,
    EcallM = 11,
    InstPageFault = 12,
    LoadPageFault = 13,
    StorePageFault = 15,
    DoubleTrap = 16,
    SoftwareCheck = 18,
    HardwareError = 19,
}

#[derive(Debug, TryFromPrimitive)]
#[repr(usize)]
pub enum Interrupt {
    Software = 0,
    Timer = 5,
    External = 9,
    Counter = 13,
}

#[derive(Debug)]
pub enum Cause {
    Trap(Trap),
    Interrupt(Interrupt),
}

pub fn read() -> usize {
    let scause: usize;
    unsafe {
        core::arch::asm!("csrr {}, scause", out(reg) scause);
    }
    scause
}

pub fn cause() -> Cause {
    let scause = read();
    if scause & (1 << 63) == 0 {
        Cause::Trap(Trap::try_from(scause).unwrap_or_else(|_| panic!("Unknown trap cause: {}", scause)))
    } else {
        Cause::Interrupt(
            Interrupt::try_from(scause & !(1 << 63)).unwrap_or_else(|_| panic!("Unknown interrupt cause: {}", scause)),
        )
    }
}
