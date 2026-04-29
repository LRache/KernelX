pub mod num {
    pub const CRMD:       u32 = 0x00;
    pub const PRMD:       u32 = 0x01;
    pub const EUEN:       u32 = 0x02;
    pub const MISC:       u32 = 0x03;
    pub const ECFG:       u32 = 0x04;
    pub const ESTAT:      u32 = 0x05;
    pub const ERA:        u32 = 0x06;
    pub const BADV:       u32 = 0x07;
    pub const BADI:       u32 = 0x08;
    pub const EENTRY:     u32 = 0x0c;
    pub const TLBIDX:     u32 = 0x10;
    pub const TLBEHI:     u32 = 0x11;
    pub const TLBELO0:    u32 = 0x12;
    pub const TLBELO1:    u32 = 0x13;
    pub const ASID:       u32 = 0x18;
    pub const PGDL:       u32 = 0x19;
    pub const PGDH:       u32 = 0x1a;
    pub const PGD:        u32 = 0x1b;
    pub const PWCL:       u32 = 0x1c;
    pub const PWCH:       u32 = 0x1d;
    pub const STLBPS:     u32 = 0x1e;
    pub const RVACFG:     u32 = 0x1f;
    pub const CPUID:      u32 = 0x20;
    pub const TCFG:       u32 = 0x41;
    pub const TVAL:       u32 = 0x42;
    pub const TICLR:      u32 = 0x44;
    pub const LLBCTL:     u32 = 0x60;
    pub const TLBRENTRY:  u32 = 0x88;
    pub const TLBRBADV:   u32 = 0x89;
    pub const TLBRERA:    u32 = 0x8a;
    pub const TLBRSAVE:   u32 = 0x8b;
    pub const TLBRELO0:   u32 = 0x8c;
    pub const TLBRELO1:   u32 = 0x8d;
    pub const TLBREHI:    u32 = 0x8e;
    pub const TLBRPRMD:   u32 = 0x8f;
    pub const DMW0:       u32 = 0x180;
    pub const DMW1:       u32 = 0x181;
    pub const DMW2:       u32 = 0x182;
    pub const DMW3:       u32 = 0x183;

    /// SAVE0 carries the current task's UserContext kaddr so
    /// `asm_usertrap_entry` can reach it atomically. Analogous to RISC-V
    /// `sscratch`. SAVE1..3 reserved.
    pub const SAVE0:      u32 = 0x30;
    pub const SAVE1:      u32 = 0x31;
    pub const SAVE2:      u32 = 0x32;
    pub const SAVE3:      u32 = 0x33;
}

/// CRMD — current mode / status. IE is the global interrupt-enable.
pub mod crmd {
    pub const PLV_MASK: usize = 0b11;
    pub const IE:  usize = 1 << 2;
    pub const DA:  usize = 1 << 3;
    pub const PG:  usize = 1 << 4;
    pub const DATF_SHIFT: usize = 5;
    pub const DATM_SHIFT: usize = 7;
}

/// PRMD — saved CRMD on exception; `ertn` restores CRMD from PRMD atomically.
pub mod prmd {
    pub const PPLV_MASK: usize = 0b11;
    pub const PIE: usize = 1 << 2;
    /// User-mode frame: PLV=3, IE=1 (user runs with interrupts on).
    pub const USERFRAME: usize = 0b11 | PIE;
}

/// PWCL — page-walk configuration (low half). Six 5-bit fields encode the
/// VA bit ranges HPTW uses at each directory level. See Vol.1 §7.4.15.
pub mod pwcl {
    pub const PTBASE_SHIFT:    u32 = 0;
    pub const PTWIDTH_SHIFT:   u32 = 5;
    pub const DIR1_BASE_SHIFT: u32 = 10;
    pub const DIR1_WIDTH_SHIFT: u32 = 15;
    pub const DIR2_BASE_SHIFT: u32 = 20;
    pub const DIR2_WIDTH_SHIFT: u32 = 25;
    pub const PTEWIDTH_SHIFT:  u32 = 30;

    /// 3-level 9-9-9-12 on 4 KiB pages with 64-bit PTEs. PTEWidth 0 = 8-byte.
    pub const THREE_LEVEL_9_9_9_12: usize =
          (12usize << PTBASE_SHIFT)
        | (9usize  << PTWIDTH_SHIFT)
        | (21usize << DIR1_BASE_SHIFT)
        | (9usize  << DIR1_WIDTH_SHIFT)
        | (30usize << DIR2_BASE_SHIFT)
        | (9usize  << DIR2_WIDTH_SHIFT)
        | (0usize  << PTEWIDTH_SHIFT);
}

/// PWCH — page-walk configuration (high half). 3 levels → 0.
pub mod pwch {
    pub const DIR3_BASE_SHIFT:  u32 = 0;
    pub const DIR3_WIDTH_SHIFT: u32 = 6;
    pub const DIR4_BASE_SHIFT:  u32 = 12;
    pub const DIR4_WIDTH_SHIFT: u32 = 18;

    pub const NONE: usize = 0;
}

/// STLBPS — shared TLB page size (log2). 4 KiB = 12.
pub mod stlbps {
    pub const PS_4K: usize = 12;
}

/// ECFG — interrupt enable and vector size.
pub mod ecfg {
    pub const LIE_SHIFT: usize = 0;
    pub const VS_SHIFT: usize = 16;
    pub const VS_MASK:  usize = 0b111 << VS_SHIFT;

    /// Bit in LIE[12:0] for the timer.
    pub const LINE_TIMER: usize = 11;
    /// Bit in LIE[12:0] for HWI0 (EIOINTC fanout on QEMU virt).
    pub const LINE_HWI0:  usize = 2;
}

/// ESTAT — decoded by the trap dispatcher.
pub mod estat {
    pub const IS_MASK:    usize = (1 << 13) - 1;
    pub const ECODE_SHIFT: usize = 16;
    pub const ECODE_MASK:  usize = 0x3f << ECODE_SHIFT;
    pub const ESUBCODE_SHIFT: usize = 22;
    pub const ESUBCODE_MASK:  usize = 0x1ff;
}

/// Known ESTAT.Ecode values.
pub mod ecode {
    pub const INT:   usize = 0x00;
    pub const PIL:   usize = 0x01; // page illegal load (NR)
    pub const PIS:   usize = 0x02; // page illegal store (NW / no-PLV)
    pub const PIF:   usize = 0x03; // page illegal fetch (NX)
    pub const PME:   usize = 0x04; // page modify (first-write D bit)
    pub const PNR:   usize = 0x05; // page not-readable
    pub const PNX:   usize = 0x06; // page not-executable
    pub const PPI:   usize = 0x07; // page privilege invalid
    pub const ADE:   usize = 0x08; // address error
    pub const ALE:   usize = 0x09; // unaligned access
    pub const BCE:   usize = 0x0a; // bound check
    pub const SYS:   usize = 0x0b; // syscall
    pub const BRK:   usize = 0x0c; // break
    pub const INE:   usize = 0x0d; // illegal instruction
    pub const IPE:   usize = 0x0e; // instruction privilege error
    pub const FPD:   usize = 0x0f; // FPU disabled
    pub const SXD:   usize = 0x10;
    pub const ASXD:  usize = 0x11;
    pub const FPE:   usize = 0x12;
}

/// TCFG — stable-timer control. Armed by a single store combining
/// InitVal (in ticks) with Periodic and En.
pub mod tcfg {
    pub const EN:       usize = 1 << 0;
    pub const PERIODIC: usize = 1 << 1;
    pub const INITVAL_SHIFT: usize = 2;
}

/// TICLR — writing bit 0 acknowledges the timer interrupt.
pub mod ticlr {
    pub const TIMER_ACK: usize = 1 << 0;
}

// -----------------------------------------------------------------------
// Read/write primitives. Usage:
//     csr::read::<{ csr::num::CRMD }>()
//     csr::write::<{ csr::num::CRMD }>(value)
// -----------------------------------------------------------------------

#[inline(always)]
pub fn read<const CSR: u32>() -> usize {
    let value: usize;
    unsafe {
        core::arch::asm!(
            "csrrd {val}, {csr}",
            val = out(reg) value,
            csr = const CSR,
            options(nostack, preserves_flags),
        );
    }
    value
}

/// Write CSR; returns the previous value (`csrwr` exchanges rd with CSR).
#[inline(always)]
pub fn write<const CSR: u32>(value: usize) -> usize {
    let old: usize;
    unsafe {
        core::arch::asm!(
            "csrwr {val}, {csr}",
            val = inout(reg) value => old,
            csr = const CSR,
            options(nostack, preserves_flags),
        );
    }
    old
}

/// `CSR = (CSR & !mask) | (value & mask)`. Returns the old CSR value.
#[inline(always)]
pub fn xchg<const CSR: u32>(value: usize, mask: usize) -> usize {
    let old: usize;
    unsafe {
        core::arch::asm!(
            "csrxchg {val}, {mask}, {csr}",
            val  = inout(reg) value => old,
            mask = in(reg) mask,
            csr  = const CSR,
            options(nostack, preserves_flags),
        );
    }
    old
}

/// Read the 64-bit stable counter (raw cycles).
#[inline(always)]
pub fn rdtime() -> u64 {
    let lo: u64;
    let _id: u64;
    unsafe {
        core::arch::asm!(
            "rdtime.d {lo}, {id}",
            lo = out(reg) lo,
            id = out(reg) _id,
            options(nostack, preserves_flags),
        );
    }
    lo
}

/// Read CPUCFG[word] — hardware feature discovery.
#[inline(always)]
pub fn cpucfg(word: u32) -> u32 {
    let value: u32;
    unsafe {
        core::arch::asm!(
            "cpucfg {val}, {word}",
            val  = out(reg) value,
            word = in(reg) word,
            options(nostack, preserves_flags),
        );
    }
    value
}

/// Stable-counter frequency (Hz) = CPUCFG[0x4] * num / den, where
/// num = CPUCFG[0x5][15:0], den = CPUCFG[0x5][31:16]. 100 MHz on QEMU virt.
pub fn stable_counter_freq() -> u64 {
    let base = cpucfg(0x4) as u64;
    let w5 = cpucfg(0x5);
    let num = (w5 & 0xffff) as u64;
    let den = ((w5 >> 16) & 0xffff) as u64;
    debug_assert!(den != 0, "CPUCFG[5] denominator is 0 — broken CPU?");
    base * num / den
}
