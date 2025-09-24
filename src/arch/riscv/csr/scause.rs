pub enum Trap {
    InstAddrMisaligned  =  0,
    InstAccessFault     =  1,
    IllegalInst         =  2,
    Breakpoint          =  3,
    LoadAddrMisaligned  =  4,
    LoadAccessFault     =  5,
    StoreAddrMisaligned =  6,
    StoreAccessFault    =  7,
    EcallU              =  8,
    EcallS              =  9,
    EcallM              = 11,
    InstPageFault       = 12,
    LoadPageFault       = 13,
    StorePageFault      = 15,
    DoubleTrap          = 16,
    SoftwareCheck       = 18,
    HardwareError       = 19,
}

pub enum Interrupt {
    Software =  0,
    Timer    =  5,
    External =  9,
    Counter  = 13,
}

pub enum Cause {
    Trap(Trap),
    Interrupt(Interrupt),
}

pub fn read() -> usize {
    let scause: usize;
    unsafe { core::arch::asm!("csrr {}, scause", out(reg) scause); }
    scause
}

pub fn cause() -> Cause {
    let scause = read();
    if scause & (1 << 63) == 0 {
        Cause::Trap(
            match scause & 0x7fffffffffffffff {
                0  => Trap::InstAddrMisaligned,
                1  => Trap::InstAccessFault,
                2  => Trap::IllegalInst,
                3  => Trap::Breakpoint,
                4  => Trap::LoadAddrMisaligned,
                5  => Trap::LoadAccessFault,
                6  => Trap::StoreAddrMisaligned,
                7  => Trap::StoreAccessFault,
                8  => Trap::EcallU,
                9  => Trap::EcallS,
                11 => Trap::EcallM,
                12 => Trap::InstPageFault,
                13 => Trap::LoadPageFault,
                15 => Trap::StorePageFault,
                16 => Trap::DoubleTrap,
                18 => Trap::SoftwareCheck,
                19 => Trap::HardwareError,
                _ => panic!("Unknown trap cause: {}", scause),
            })
    } else {
        match scause & 0x7fffffffffffffff {
             0 => Cause::Interrupt(Interrupt::Software),
             5 => Cause::Interrupt(Interrupt::Timer),
             9 => Cause::Interrupt(Interrupt::External),
            13 => Cause::Interrupt(Interrupt::Counter),
             _ => panic!("Unknown interrupt cause: {}", scause),
                
        }
    }
}
