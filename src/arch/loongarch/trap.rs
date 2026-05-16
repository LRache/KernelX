use crate::arch::loongarch::{csr, eiointc, pch_pic};
use crate::kernel::trap;
use crate::{kinfo, kwarn};

#[unsafe(no_mangle)]
pub extern "C" fn kerneltrap_handler() {
    let estat = csr::read::<{ csr::num::ESTAT }>();
    let ecode = (estat & csr::estat::ECODE_MASK) >> csr::estat::ECODE_SHIFT;
    let is = estat & csr::estat::IS_MASK;
    let era = csr::read::<{ csr::num::ERA }>();

    if ecode == csr::ecode::INT {
        if is & (1 << csr::ecfg::LINE_TIMER) != 0 {
            csr::write::<{ csr::num::TICLR }>(csr::ticlr::TIMER_ACK);
            trap::timer_interrupt();
        }

        if is & (1 << csr::ecfg::LINE_HWI0) != 0 {
            while let Some(irq) = eiointc::claim_irq() {
                trap::external_interrupt(irq);
                eiointc::complete_irq(irq);
                pch_pic::ack_irq(irq);
            }
        }

        let other = is & !(1 << csr::ecfg::LINE_TIMER) & !(1 << csr::ecfg::LINE_HWI0);
        if other != 0 {
            kwarn!("loongarch: unroutable interrupt lines {:#x} @ ERA={:#x}", other, era);
        }
        return;
    }

    // Synchronous exception from kernel mode — always a bug.
    let badv = csr::read::<{ csr::num::BADV }>();
    let badi = csr::read::<{ csr::num::BADI }>();
    panic!(
        "loongarch: unhandled kernel trap: Ecode={:#x} ({}), \
         ESTAT={:#x}, ERA={:#x}, BADV={:#x}, BADI={:#x}",
        ecode,
        ecode_name(ecode),
        estat,
        era,
        badv,
        badi,
    );
}

fn ecode_name(ecode: usize) -> &'static str {
    match ecode {
        csr::ecode::INT => "INT",
        csr::ecode::PIL => "PIL (load, NR)",
        csr::ecode::PIS => "PIS (store, no-PLV / NW)",
        csr::ecode::PIF => "PIF (fetch, NX)",
        csr::ecode::PME => "PME (first-write dirty)",
        csr::ecode::PNR => "PNR (page NR)",
        csr::ecode::PNX => "PNX (page NX)",
        csr::ecode::PPI => "PPI (privilege)",
        csr::ecode::ADE => "ADE (addr error)",
        csr::ecode::ALE => "ALE (unaligned)",
        csr::ecode::BCE => "BCE",
        csr::ecode::SYS => "SYS (syscall)",
        csr::ecode::BRK => "BRK (break)",
        csr::ecode::INE => "INE (illegal inst)",
        csr::ecode::IPE => "IPE (inst priv err)",
        csr::ecode::FPD => "FPD (FPU disabled)",
        csr::ecode::SXD => "SXD",
        csr::ecode::ASXD => "ASXD",
        csr::ecode::FPE => "FPE",
        _ => "unknown",
    }
}

pub fn install_trap_entry() {
    unsafe extern "C" {
        fn asm_kerneltrap_entry() -> !;
        fn asm_tlb_refill_entry() -> !;
    }

    let entry_addr = asm_kerneltrap_entry as *const() as usize;
    debug_assert!(
        entry_addr & 0xfff == 0,
        "EENTRY must be page-aligned, got {:#x}",
        entry_addr,
    );

    csr::write::<{ csr::num::EENTRY }>(entry_addr);
    csr::xchg::<{ csr::num::ECFG }>(0, csr::ecfg::VS_MASK);

    let refill_va = asm_tlb_refill_entry as *const() as usize;
    let refill_pa = crate::arch::kaddr_to_paddr(refill_va);
    debug_assert!(
        refill_pa & 0xfff == 0,
        "TLBRENTRY must be 4 KiB-aligned, got {:#x}",
        refill_pa,
    );
    csr::write::<{ csr::num::TLBRENTRY }>(refill_pa);

    kinfo!(
        "loongarch: EENTRY = {:#x} (single-entry, Ecode-dispatched), \
         TLBRENTRY = {:#x} (PA of refill handler at VA {:#x})",
        entry_addr,
        refill_pa,
        refill_va,
    );
}
