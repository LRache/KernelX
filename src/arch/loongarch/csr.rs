//! LoongArch CSR helpers.
//!
//! Why a whole file for this: `csrrd`/`csrwr`/`csrxchg` need **immediate**
//! CSR numbers encoded into the instruction, so we cannot just take a
//! `csr: usize` runtime argument — each CSR we touch needs its own tiny
//! read/write function. The macros below generate those; everything else
//! in `src/arch/loongarch/*` calls into here instead of embedding inline
//! asm directly, keeping each instance of `csrwr` in one place.

/// CSR numbers we care about in Phase 4. Kept as named constants so that
/// reading the trap/timer code doesn't require the ISA manual open.
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
    pub const DMW0:       u32 = 0x180;
    pub const DMW1:       u32 = 0x181;
    pub const DMW2:       u32 = 0x182;
    pub const DMW3:       u32 = 0x183;
}

// ---------------------------------------------------------------------
// Bit layouts for the CSRs we read or construct in software.
// ---------------------------------------------------------------------

/// CRMD — current mode and status. `IE` is the global interrupt-enable.
pub mod crmd {
    /// Privilege level (0..3) — bits 1:0.
    pub const PLV_MASK: usize = 0b11;
    /// Interrupt-enable bit.
    pub const IE:  usize = 1 << 2;
    /// Direct-address translation enable.
    pub const DA:  usize = 1 << 3;
    /// Paging enable.
    pub const PG:  usize = 1 << 4;
    /// Data fetch memory-access type (bits 6:5).
    pub const DATF_SHIFT: usize = 5;
    /// Instruction fetch memory-access type (bits 8:7).
    pub const DATM_SHIFT: usize = 7;
}

/// ECFG — interrupt enable and vector size.
pub mod ecfg {
    /// Per-line enable bits IS[12:0]; we care about bit 11 (timer) and
    /// bit 10 (HWI9, the routed PCI/virtio line on QEMU virt).
    pub const LIE_SHIFT: usize = 0;
    /// VS field (bits 18:16) — exception vector size. 0 = single entry.
    pub const VS_SHIFT: usize = 16;
    pub const VS_MASK:  usize = 0b111 << VS_SHIFT;

    /// Line indices inside LIE[12:0].
    pub const LINE_TIMER: usize = 11;
    /// QEMU virt routes the PCI / ns16550a / rtc through HWI0 (bit 2).
    /// Kept as a constant even though Phase 4 doesn't subscribe to it yet.
    pub const LINE_HWI0:  usize = 2;
}

/// ESTAT — decoded by the trap dispatcher.
pub mod estat {
    /// Pending-interrupt bits IS[14:0].
    pub const IS_MASK:    usize = (1 << 13) - 1;
    /// Ecode field (bits 21:16). See `ecode` module for values.
    pub const ECODE_SHIFT: usize = 16;
    pub const ECODE_MASK:  usize = 0x3f << ECODE_SHIFT;
    /// EsubCode field (bits 30:22).
    pub const ESUBCODE_SHIFT: usize = 22;
    pub const ESUBCODE_MASK:  usize = 0x1ff << ESUBCODE_SHIFT;
}

/// Known ESTAT.Ecode values. Unused variants are Phase 5 territory but we
/// define them up front so the dispatcher match is exhaustive enough to
/// panic with a useful string instead of a number.
pub mod ecode {
    pub const INT:   usize = 0x00; // interrupt (any of IS[12:0])
    pub const PIL:   usize = 0x01; // page illegal load (NR)
    pub const PIS:   usize = 0x02; // page illegal store (NW / no-PLV)
    pub const PIF:   usize = 0x03; // page illegal fetch (NX)
    pub const PME:   usize = 0x04; // page modify exception (first-write D bit)
    pub const PNR:   usize = 0x05; // page not-readable (bit NR set)
    pub const PNX:   usize = 0x06; // page not-executable
    pub const PPI:   usize = 0x07; // page privilege invalid
    pub const ADE:   usize = 0x08; // address error (alignment)
    pub const ALE:   usize = 0x09; // unaligned access
    pub const BCE:   usize = 0x0a; // bound check
    pub const SYS:   usize = 0x0b; // syscall instruction
    pub const BRK:   usize = 0x0c; // break
    pub const INE:   usize = 0x0d; // illegal instruction
    pub const IPE:   usize = 0x0e; // instruction privilege error
    pub const FPD:   usize = 0x0f; // FPU disabled
    pub const SXD:   usize = 0x10; // SX disabled
    pub const ASXD:  usize = 0x11; // ASX disabled
    pub const FPE:   usize = 0x12; // FP exception
}

/// TCFG — stable-timer control. Timer is ARMed by writing `{InitVal, Periodic, En}`
/// in one store; InitVal occupies bits [63:2] so the count is in units of a
/// single clock tick (we multiply by (freq/1e6) when programming interval-in-us).
pub mod tcfg {
    pub const EN:       usize = 1 << 0;
    pub const PERIODIC: usize = 1 << 1;
    pub const INITVAL_SHIFT: usize = 2;
}

/// TICLR — writing bit 0 acknowledges the timer interrupt.
pub mod ticlr {
    pub const TIMER_ACK: usize = 1 << 0;
}

// ---------------------------------------------------------------------
// The read/write primitives. `csrrd`/`csrwr`/`csrxchg` require an immediate
// CSR number, so these are generated per-CSR via macros.
//
// Usage:
//     csr::read::<{ csr::num::CRMD }>()
//     csr::write::<{ csr::num::CRMD }>(value)
// ...which expands to the appropriate `csrrd`/`csrwr`.
//
// Rust's const-generic + const-eval is enough to inline the immediate at
// monomorphisation, so this is zero-overhead.
// ---------------------------------------------------------------------

/// Read CSR with const-immediate number. Every call site is monomorphised
/// with the CSR number baked into the instruction.
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

/// Write CSR with const-immediate number. Returns the PREVIOUS value (per
/// the ISA: `csrwr` exchanges rd with the CSR).
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

/// Exchange a set of bits in the CSR: `csr = (csr & !mask) | (value & mask)`.
/// Returns the OLD value of the CSR.
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

/// Read the 64-bit stable counter. Returns raw cycles — the caller converts
/// to microseconds using the frequency CPUCFG reports.
#[inline(always)]
pub fn rdtime() -> u64 {
    let lo: u64;
    let _id: u64;
    // `rdtime.d rd, rj` — rd <- counter, rj <- stable-counter ID (we ignore).
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

/// Read CPUCFG[word] to discover hardware features. We use `0x4` for the
/// stable-counter frequency base and `0x5` for the numerator/denominator.
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

/// Compute the stable-counter frequency (Hz).
///
/// Per the ISA: f = base * (numerator / denominator), with:
///   - base:        CPUCFG[0x4]
///   - numerator:   CPUCFG[0x5][15:0]
///   - denominator: CPUCFG[0x5][31:16]
///
/// On QEMU la464 this usually lands at 100 MHz (clock_freq=0x5f5e100 in the
/// DTS). Kept as a function because the number shows up in
/// `set_next_time_event_us` and in the `get_time_us` unit conversion.
pub fn stable_counter_freq() -> u64 {
    let base = cpucfg(0x4) as u64;
    let w5 = cpucfg(0x5);
    let num = (w5 & 0xffff) as u64;
    let den = ((w5 >> 16) & 0xffff) as u64;
    debug_assert!(den != 0, "CPUCFG[5] denominator is 0 — broken CPU?");
    base * num / den
}
