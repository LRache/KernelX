#[cfg(feature = "debug_pagetable")]
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::kernel::scheduler::current;

#[cfg(not(feature = "no-smp"))]
use super::sbi_driver;

#[cfg(feature = "debug_pagetable")]
static TLB_CONTEXT_IDS: [AtomicUsize; usize::BITS as usize] = [const { AtomicUsize::new(0) }; usize::BITS as usize];

pub fn flush_tlb_local() {
    // SAFETY: Callers publish page-table writes before invoking this function.
    // The fence orders those writes before invalidating every local translation.
    unsafe {
        core::arch::asm!(
            "fence rw, rw",
            "sfence.vma zero, zero",
            options(nostack, preserves_flags)
        )
    };
}

pub fn flush_tlb_all() {
    #[cfg(feature = "no-smp")]
    {
        flush_tlb_local();
        return;
    }

    #[cfg(not(feature = "no-smp"))]
    if super::core_count() < 2 {
        flush_tlb_local();
        return;
    }

    #[cfg(not(feature = "no-smp"))]
    // SAFETY: Page-table writes are complete before this function is called.
    // Publish them before the synchronous SBI RFENCE.
    unsafe {
        core::arch::asm!("fence rw, rw", options(nostack, preserves_flags))
    };

    #[cfg(not(feature = "no-smp"))]
    sbi_driver::remote_sfence_vma_all().unwrap_or_else(|error| panic!("SBI remote SFENCE.VMA failed: {error}"));
}

pub fn flush_tlb_cpu_mask(cpu_mask: usize) {
    if cpu_mask == 0 {
        return;
    }

    let valid_cpu_mask = super::cpu::hart_mask();
    debug_assert_eq!(
        cpu_mask & !valid_cpu_mask,
        0,
        "TLB flush mask contains an unavailable hart"
    );
    let cpu_mask = cpu_mask & valid_cpu_mask;
    let current_cpu = 1usize
        .checked_shl(current::hart_id().try_into().expect("hart ID does not fit in u32"))
        .expect("hart ID exceeds TLB CPU mask width");

    if cpu_mask & !current_cpu == 0 {
        flush_tlb_local();
        return;
    }

    // SAFETY: Page-table writes are complete before this function is called.
    // Publish them before the synchronous SBI RFENCE.
    unsafe { core::arch::asm!("fence rw, rw", options(nostack, preserves_flags)) };

    #[cfg(not(feature = "no-smp"))]
    sbi_driver::remote_sfence_vma(cpu_mask)
        .unwrap_or_else(|error| panic!("SBI targeted remote SFENCE.VMA failed: {error}"));

    #[cfg(feature = "no-smp")]
    unreachable!("single-core TLB flush contains a remote hart");
}

#[cfg(feature = "debug_pagetable")]
pub fn invalidate_tlb_context(hartid: usize, expected_context_id: Option<usize>) {
    let context_id = TLB_CONTEXT_IDS[hartid].load(Ordering::Acquire);
    match expected_context_id {
        Some(expected_context_id) => assert_eq!(
            context_id, expected_context_id,
            "cached page table does not match the hart TLB context"
        ),
        None => assert_eq!(
            context_id, 0,
            "hart has a TLB context without a matching cached page table"
        ),
    }
    TLB_CONTEXT_IDS[hartid].store(0, Ordering::Release);
}

#[cfg(feature = "debug_pagetable")]
pub fn validate_activated_tlb_context(hartid: usize, context_id: usize) {
    let cached_context_id = TLB_CONTEXT_IDS[hartid].load(Ordering::Acquire);
    if cached_context_id == 0 {
        TLB_CONTEXT_IDS[hartid].store(context_id, Ordering::Release);
    } else {
        assert_eq!(
            cached_context_id, context_id,
            "returning to a page table without invalidating another TLB context"
        );
    }
}
