use core::hint::spin_loop;
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

static TLB_FLUSH_REQUESTS: [AtomicUsize; usize::BITS as usize] = [const { AtomicUsize::new(0) }; usize::BITS as usize];
static TLB_FLUSH_COMPLETIONS: [AtomicUsize; usize::BITS as usize] =
    [const { AtomicUsize::new(0) }; usize::BITS as usize];

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
    let request = TLB_FLUSH_REQUESTS[hartid].load(Ordering::Acquire);
    // SAFETY: The acquire above observes page-table writes published by the
    // requester before this local invalidation. The completion release makes
    // the finished INVTLB visible to the waiting CPU.
    unsafe {
        core::arch::asm!(
            "dbar 0",
            "invtlb 0x00, $zero, $zero",
            "dbar 0",
            "ibar 0",
            options(nostack, preserves_flags)
        );
    }
    TLB_FLUSH_COMPLETIONS[hartid].store(request, Ordering::Release);
}

pub fn flush_tlb_cpu_mask(cpu_mask: usize) {
    let mut requests = [0usize; usize::BITS as usize];
    let mut targets = cpu_mask;
    while targets != 0 {
        let hartid = targets.trailing_zeros() as usize;
        requests[hartid] = TLB_FLUSH_REQUESTS[hartid]
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        send_ipi(hartid, IpiVector::TlbFlush);
        targets &= targets - 1;
    }

    let mut targets = cpu_mask;
    while targets != 0 {
        let hartid = targets.trailing_zeros() as usize;
        while TLB_FLUSH_COMPLETIONS[hartid].load(Ordering::Acquire) < requests[hartid] {
            spin_loop();
        }
        targets &= targets - 1;
    }
}

pub fn send_ipi(cpu: usize, vector: IpiVector) {
    let cpu = u32::try_from(cpu).expect("LoongArch CPU ID does not fit in u32");
    let value = SEND_BLOCKING as u32 | (cpu << SEND_CPU_SHIFT) | vector as u32;
    iocsr_write_w(reg::IPI_SEND, value);
}
