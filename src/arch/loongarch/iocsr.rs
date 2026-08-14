use core::sync::atomic::{AtomicUsize, Ordering};

mod reg {
    pub const IPI_STATUS: usize = 0x1000;
    pub const IPI_ENABLE: usize = 0x1004;
    pub const IPI_CLEAR: usize = 0x100c;
    pub const IPI_SEND: usize = 0x1040;
    pub const MAILBOX_SEND: usize = 0x1048;
}

const SEND_CPU_SHIFT: usize = 16;
const SEND_BLOCKING: u64 = 1 << 31;

const TLB_FLUSH_ACTIVE: usize = 1 << (usize::BITS - 1);
const TLB_FLUSH_PENDING: usize = 1 << (usize::BITS - 2);
const TLB_FLUSH_SEQUENCE_MASK: usize = TLB_FLUSH_PENDING - 1;
const TLB_FLUSH_SEQUENCE_HALF: usize = 1 << (usize::BITS - 3);

static TLB_FLUSH_STATES: [AtomicUsize; usize::BITS as usize] =
    [const { AtomicUsize::new(TLB_FLUSH_PENDING) }; usize::BITS as usize];
static TLB_FLUSH_COMPLETIONS: [AtomicUsize; usize::BITS as usize] =
    [const { AtomicUsize::new(0) }; usize::BITS as usize];

#[cfg(feature = "debug_pagetable")]
static TLB_CONTEXT_IDS: [AtomicUsize; usize::BITS as usize] = [const { AtomicUsize::new(0) }; usize::BITS as usize];

#[derive(Clone, Copy)]
#[repr(u64)]
enum Mailbox {
    Entry = 0,
    Stack = 1,
}

#[derive(Clone, Copy)]
#[repr(u32)]
pub enum IpiVector {
    Wake = 0,
    TlbFlush = 1,
}

impl IpiVector {
    const fn mask(self) -> u32 {
        1 << self as u32
    }
}

#[inline]
pub fn iocsr_read_d(offset: usize) -> u64 {
    let v: u64;
    unsafe {
        core::arch::asm!(
            "iocsrrd.d {v}, {off}",
            v = out(reg) v,
            off = in(reg) offset,
            options(nostack, preserves_flags),
        );
    }
    v
}

/// Write a 64-bit IOCSR register.
#[inline]
pub fn iocsr_write_d(offset: usize, value: u64) {
    unsafe {
        core::arch::asm!(
            "iocsrwr.d {v}, {off}",
            v = in(reg) value,
            off = in(reg) offset,
            options(nostack, preserves_flags),
        );
    }
}

/// Read a 32-bit IOCSR register.
#[inline]
#[allow(dead_code)]
pub fn iocsr_read_w(offset: usize) -> u32 {
    let v: u32;
    unsafe {
        core::arch::asm!(
            "iocsrrd.w {v}, {off}",
            v = out(reg) v,
            off = in(reg) offset,
            options(nostack, preserves_flags),
        );
    }
    v
}

/// Write a 32-bit IOCSR register.
#[inline]
#[allow(dead_code)]
pub fn iocsr_write_w(offset: usize, value: u32) {
    unsafe {
        core::arch::asm!(
            "iocsrwr.w {v}, {off}",
            v = in(reg) value,
            off = in(reg) offset,
            options(nostack, preserves_flags),
        );
    }
}

/// Write a single byte (for per-IRQ route/vec tables).
#[inline]
pub fn iocsr_write_b(offset: usize, value: u8) {
    unsafe {
        core::arch::asm!(
            "iocsrwr.b {v}, {off}",
            v = in(reg) value as u32,
            off = in(reg) offset,
            options(nostack, preserves_flags),
        );
    }
}

fn mailbox_send(cpu: usize, mailbox: Mailbox, data: usize) {
    let cpu = u64::try_from(cpu).expect("LoongArch CPU ID does not fit in u64");
    let mailbox = mailbox as u64;
    let data = data as u64;

    let high = SEND_BLOCKING | (cpu << SEND_CPU_SHIFT) | ((mailbox * 2 + 1) << 2) | (data & 0xffff_ffff_0000_0000);
    iocsr_write_d(reg::MAILBOX_SEND, high);

    let low = SEND_BLOCKING | (cpu << SEND_CPU_SHIFT) | (mailbox * 2 << 2) | (data << 32);
    iocsr_write_d(reg::MAILBOX_SEND, low);
}

pub fn start_core(cpu: usize, entry: usize, stack_top: usize) {
    mailbox_send(cpu, Mailbox::Stack, stack_top);
    mailbox_send(cpu, Mailbox::Entry, entry);
    send_ipi(cpu, IpiVector::Wake);
}

pub fn enable_ipi() {
    let _ = acknowledge_ipi();
    iocsr_write_w(reg::IPI_ENABLE, IpiVector::Wake.mask() | IpiVector::TlbFlush.mask());
}

pub fn acknowledge_ipi() -> u32 {
    let status = iocsr_read_w(reg::IPI_STATUS);
    if status != 0 {
        iocsr_write_w(reg::IPI_CLEAR, status);
        // SAFETY: `dbar 0` waits for the local IOCSR clear write to complete
        // before the interrupt handler returns.
        unsafe { core::arch::asm!("dbar 0", options(nostack, preserves_flags)) };
    }
    status
}

pub fn handle_ipi() {
    let status = acknowledge_ipi();
    if status & IpiVector::TlbFlush.mask() == 0 {
        return;
    }

    let hartid = crate::kernel::scheduler::current::hart_id();
    handle_tlb_flush_requests(hartid);
}

fn tlb_flush_sequence_reached(completion: usize, request: usize) -> bool {
    completion.wrapping_sub(request) & TLB_FLUSH_SEQUENCE_MASK < TLB_FLUSH_SEQUENCE_HALF
}

fn handle_tlb_flush_requests(hartid: usize) {
    loop {
        let request = TLB_FLUSH_STATES[hartid].load(Ordering::Acquire) & TLB_FLUSH_SEQUENCE_MASK;
        let completion = TLB_FLUSH_COMPLETIONS[hartid].load(Ordering::Relaxed);
        if tlb_flush_sequence_reached(completion, request) {
            return;
        }

        crate::arch::flush_tlb_all();

        // Mark the flush as completed.
        TLB_FLUSH_COMPLETIONS[hartid].store(request, Ordering::Release);
    }
}

pub fn deactivate_tlb_cpu(hartid: usize) {
    // SAFETY: User execution has stopped on this CPU. Drain its earlier memory
    // accesses before publishing the inactive state to a requester that may
    // release a formerly mapped physical page.
    unsafe { core::arch::asm!("dbar 0", options(nostack, preserves_flags)) };

    let previous = TLB_FLUSH_STATES[hartid].fetch_and(!TLB_FLUSH_ACTIVE, Ordering::AcqRel);
    debug_assert_ne!(previous & TLB_FLUSH_ACTIVE, 0, "deactivating an inactive TLB CPU");

    let request = previous & TLB_FLUSH_SEQUENCE_MASK;
    let completion = TLB_FLUSH_COMPLETIONS[hartid].load(Ordering::Relaxed);
    if !tlb_flush_sequence_reached(completion, request) {
        // The CPU is now quiescent with respect to user translations. Preserve
        // the invalidation for the next user return, but let a requester that
        // raced with trap entry release the old mapping immediately.
        TLB_FLUSH_STATES[hartid].fetch_or(TLB_FLUSH_PENDING, Ordering::Relaxed);
        TLB_FLUSH_COMPLETIONS[hartid].store(request, Ordering::Release);
    }
}

pub fn mark_tlb_flush_pending_for_switch(hartid: usize) {
    let previous = TLB_FLUSH_STATES[hartid].fetch_or(TLB_FLUSH_PENDING, Ordering::AcqRel);
    debug_assert_eq!(previous & TLB_FLUSH_ACTIVE, 0, "switching an active TLB CPU");
}

pub fn activate_tlb_cpu(hartid: usize) -> bool {
    let state = &TLB_FLUSH_STATES[hartid];
    let mut previous = state.load(Ordering::Acquire);
    let mut flushed = false;

    loop {
        debug_assert_eq!(previous & TLB_FLUSH_ACTIVE, 0, "activating an active TLB CPU");

        if previous & TLB_FLUSH_PENDING != 0 {
            crate::arch::flush_tlb_all();
            flushed = true;
            TLB_FLUSH_COMPLETIONS[hartid].store(previous & TLB_FLUSH_SEQUENCE_MASK, Ordering::Release);
        }

        let next = TLB_FLUSH_ACTIVE | (previous & TLB_FLUSH_SEQUENCE_MASK);
        match state.compare_exchange(previous, next, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => {
                if !flushed {
                    debug_assert!(tlb_flush_sequence_reached(
                        TLB_FLUSH_COMPLETIONS[hartid].load(Ordering::Relaxed),
                        previous & TLB_FLUSH_SEQUENCE_MASK
                    ));
                }
                return flushed;
            }
            Err(current) => previous = current,
        }
    }
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
pub fn validate_activated_tlb_context(hartid: usize, context_id: usize, flushed: bool) {
    if flushed {
        TLB_CONTEXT_IDS[hartid].store(context_id, Ordering::Release);
    } else {
        assert_eq!(
            TLB_CONTEXT_IDS[hartid].load(Ordering::Acquire),
            context_id,
            "returning to a page table without invalidating another TLB context"
        );
    }
}

fn request_tlb_flush(hartid: usize) -> Option<usize> {
    let state = &TLB_FLUSH_STATES[hartid];
    let mut previous = state.load(Ordering::Acquire);

    loop {
        let request = (previous & TLB_FLUSH_SEQUENCE_MASK).wrapping_add(1) & TLB_FLUSH_SEQUENCE_MASK;
        let active = previous & TLB_FLUSH_ACTIVE != 0;
        let next = if active {
            TLB_FLUSH_ACTIVE | (previous & TLB_FLUSH_PENDING) | request
        } else {
            TLB_FLUSH_PENDING | request
        };
        match state.compare_exchange_weak(previous, next, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return active.then_some(request),
            Err(current) => previous = current,
        }
    }
}

pub fn flush_tlb_cpu_mask(cpu_mask: usize) {
    let mut requests = [0usize; usize::BITS as usize];
    let mut requested_targets = 0usize;
    let mut targets = cpu_mask;
    while targets != 0 {
        let hartid = targets.trailing_zeros() as usize;
        if let Some(request) = request_tlb_flush(hartid) {
            requests[hartid] = request;
            requested_targets |= 1usize << hartid;
            send_ipi(hartid, IpiVector::TlbFlush);
        }
        targets &= targets - 1;
    }

    let mut targets = requested_targets;
    while targets != 0 {
        let hartid = targets.trailing_zeros() as usize;
        while !tlb_flush_sequence_reached(TLB_FLUSH_COMPLETIONS[hartid].load(Ordering::Acquire), requests[hartid]) {
            core::hint::spin_loop();
        }
        targets &= targets - 1;
    }
}

pub fn send_ipi(cpu: usize, vector: IpiVector) {
    let cpu = u32::try_from(cpu).expect("LoongArch CPU ID does not fit in u32");
    let value = SEND_BLOCKING as u32 | (cpu << SEND_CPU_SHIFT) | vector as u32;
    iocsr_write_w(reg::IPI_SEND, value);
}
