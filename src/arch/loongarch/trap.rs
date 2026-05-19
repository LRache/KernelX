use crate::arch::loongarch::csr;
use crate::kinfo;

pub fn install_trap_entry() {
    unsafe extern "C" {
        fn asm_kerneltrap_entry() -> !;
        fn asm_tlb_refill_entry() -> !;
    }

    let entry_addr = asm_kerneltrap_entry as *const () as usize;
    debug_assert!(
        entry_addr & 0xfff == 0,
        "EENTRY must be page-aligned, got {:#x}",
        entry_addr,
    );

    csr::write::<{ csr::num::EENTRY }>(entry_addr);
    csr::xchg::<{ csr::num::ECFG }>(0, csr::ecfg::VS_MASK);

    let refill_va = asm_tlb_refill_entry as *const () as usize;
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
