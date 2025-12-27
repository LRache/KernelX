use crate::kernel::mm::page;
use crate::klib::{SpinLock, InitedCell};
use crate::arch;

mod reg {
    pub const PRIORITY : usize = 0x0;
    // pub const PENDING  : usize = 0x1000;
    pub const SENABLE  : usize = 0x2080;
    pub const SPRIORITY: usize = 0x201000;
    pub const SCLAIM   : usize = 0x201004;
}

struct PLIC {
    base: usize,
}

impl PLIC {
    fn new(base: usize) -> Self {
        Self { base }
    }

    fn read(&self, offset: usize) -> u32 {
        unsafe { arch::read_volatile((self.base + offset) as *const u32) }
    }

    fn write(&mut self, offset: usize, value: u32) {
        unsafe { arch::write_volatile((self.base + offset) as *mut u32, value) }
    }

    fn set_hart_threshold(&mut self, hart_id: usize, threshold: u32) {
        let spriority = reg::SPRIORITY + hart_id * 0x2000;
        self.write(spriority, threshold);
    }

    fn enable_irq_for_hart(&mut self, hart_id: usize, irq: u32) {
        let index = (irq / 32) as usize;
        let bit = irq % 32;
        let senable = reg::SENABLE + hart_id * 0x100 + index * 4;
        self.write(senable, self.read(senable) | (1 << bit));
    }

    fn set_irq_priority(&mut self, irq: u32, priority: u32) {
        let offset = reg::PRIORITY + irq as usize * 4;
        self.write(offset, priority);
    }

    fn claim_irq(&mut self, hart_id: usize) -> u32 {
        let sclaim = reg::SCLAIM + hart_id * 0x2000;
        self.read(sclaim)
    }

    fn complete_irq(&mut self, hart_id: usize, irq: u32) {
        let sclaim = reg::SCLAIM + hart_id * 0x2000;
        self.write(sclaim, irq);
    }
}

static PLIC: InitedCell<Option<SpinLock<PLIC>>> = InitedCell::uninit();

pub fn init(base: usize, size: usize) {
    let pages = arch::page_count(size);
    let kbase = page::alloc_contiguous(pages);
    arch::map_kernel_addr(kbase, base, size, arch::MapPerm::RW);
    
    let plic = PLIC::new(kbase);
    PLIC.init(Some(SpinLock::new(plic)));
}

pub fn not_found() {
    PLIC.init(None);
}

pub fn enable_interrupt_for_all_harts() {
    if let Some(ref plic) = *PLIC {
        let mut plic = plic.lock();
        for hart_id in 0..arch::riscv::core_count() {
            plic.set_hart_threshold(hart_id, 0);
        }
    }
}

pub fn enable_irq_for_all_harts(irq: u32) {
    if let Some(ref plic) = *PLIC {
        let mut plic = plic.lock();
        for hart_id in 0..arch::riscv::core_count() {
            plic.enable_irq_for_hart(hart_id, irq);
        }
        plic.set_irq_priority(irq, 1);
    }
}

pub fn claim_irq(hart_id: usize) -> Option<u32> {
    if let Some(ref plic) = *PLIC {
        let mut plic = plic.lock();
        Some(plic.claim_irq(hart_id))
    } else {
        None
    }
}

pub fn complete_irq(hart_id: usize, irq: u32) {
    if let Some(ref plic) = *PLIC {
        let mut plic = plic.lock();
        plic.complete_irq(hart_id, irq);
    }
}
