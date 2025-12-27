use alloc::collections::btree_map::BTreeMap;
use fdt::Fdt;
use fdt::node::FdtNode;

use crate::kernel::mm::page;
use crate::klib::{SpinLock, InitedCell};
use crate::{arch, kinfo, kwarn};

mod reg {
    pub const PRIORITY : usize = 0x0;
    // pub const PENDING  : usize = 0x1000;
    pub const SENABLE  : usize = 0x2000;
    pub const SPRIORITY: usize = 0x200000;
    pub const SCLAIM   : usize = 0x200004;
}
struct PLIC {
    base: usize,
    smode_context: BTreeMap<usize, usize>, // hart_id -> context_id
}

impl PLIC {
    fn new(base: usize, smode_context: BTreeMap<usize, usize>) -> Self {
        Self { base, smode_context }
    }

    fn read(&self, offset: usize) -> u32 {
        unsafe { arch::read_volatile((self.base + offset) as *const u32) }
    }

    fn write(&self, offset: usize, value: u32) {
        unsafe { arch::write_volatile((self.base + offset) as *mut u32, value) }
    }

    fn get_context_id(&self, hart_id: usize) -> Option<usize> {
        self.smode_context.get(&hart_id).copied()
    }

    fn set_context_threshold(&self, context_id: usize, threshold: u32) {
        let spriority = reg::SPRIORITY + context_id * 0x1000;
        self.write(spriority, threshold);
    }

    pub fn set_hart_threshold(&mut self, hart_id: usize, threshold: u32) {
        self.get_context_id(hart_id).map(|context_id| {
            self.set_context_threshold(context_id, threshold);
        });
    }

    fn enable_irq_for_context(&self, context_id: usize, irq: u32) {
        let index = (irq / 32) as usize;
        let bit = irq % 32;
        let senable = reg::SENABLE + context_id * 0x80 + index * 4;
        self.write(senable, self.read(senable) | (1 << bit));
    }

    fn enable_irq_for_all_harts(&mut self, irq: u32) {
        self.smode_context.iter().for_each(|(_, &context_id)| {
            self.enable_irq_for_context(context_id, irq);
        });
    }

    pub fn set_irq_priority(&mut self, irq: u32, priority: u32) {
        let offset = reg::PRIORITY + irq as usize * 4;
        self.write(offset, priority);
    }

    fn claim_context_irq(&self, context_id: usize) -> u32 {
        let sclaim = reg::SCLAIM + context_id * 0x1000;
        self.read(sclaim)
    }

    pub fn claim_irq(&mut self, hart_id: usize) -> Option<u32> {
        self.get_context_id(hart_id).map(|context_id| {
            self.claim_context_irq(context_id)
        })
    }

    fn complete_context_irq(&self, context_id: usize, irq: u32) {
        let sclaim = reg::SCLAIM + context_id * 0x1000;
        self.write(sclaim, irq);
    }
    
    pub fn complete_irq(&mut self, hart_id: usize, irq: u32) {
        let context_id = self.get_context_id(hart_id).unwrap();
        self.complete_context_irq(context_id, irq);
    }
}

static PLIC: InitedCell<Option<SpinLock<PLIC>>> = InitedCell::uninit();

pub fn from_fdt(fdt: &Fdt, fdt_node: &FdtNode) {
    let helper = || {
        let mut reg_prop = fdt_node.reg()?;
        let reg = reg_prop.next()?;
        let base = reg.starting_address as usize;
        let size = reg.size? as usize;
        let cell_size = fdt_node.interrupt_cells()?;

        if let Some(interrupts_extended_prop) = fdt_node.property("interrupts-extended") {
            let length = interrupts_extended_prop.value.len();
            if length % (cell_size * 2) != 0 {
                return None;
            }

            let mut smode_context = BTreeMap::new();
            let mut cursor = 0;
            let mut i = 0;
            while cursor < length {
                let slice = &interrupts_extended_prop.value[cursor..cursor + 8];
                kinfo!("PLIC: interrupts-extended slice: {:x?}\n", slice);
                let phandle = u32::from_be_bytes(slice[0..4].try_into().unwrap());
                let hwirq = u32::from_be_bytes(slice[4..8].try_into().unwrap());

                kinfo!("PLIC: Found context {} for phandle {} interrupt number {}", i, phandle, hwirq);
                
                if hwirq == 0x9 { // S-mode context
                    let interrupt_controller_node = fdt.find_phandle(phandle)?;
                    let cpu_node = interrupt_controller_node.parent()?;

                    let get_hart_id = || {
                        let compatible = cpu_node.compatible()?;
                        if !compatible.all().into_iter().any(|c| c == "riscv") {
                            return None;
                        }

                        if cpu_node.property("device_type")?.as_str()? != "cpu" {
                            return None;
                        }

                        Some(cpu_node.property("reg")?.as_usize()?)
                    };

                    if let Some(hart_id) = get_hart_id() {
                        smode_context.insert(hart_id, i);
                    } else {
                        kwarn!("PLIC: Failed to get hart ID for PLIC context {}\n", i);
                        return None;
                    }
                }

                cursor += 8;
                i += 1;
            }

            let pages = arch::page_count(size);
            let kbase = page::alloc_contiguous(pages);
            arch::map_kernel_addr(kbase, base, size, arch::MapPerm::RW);

            return Some(PLIC::new(kbase, smode_context))
        } else {
            return None;
        };
    };
    
    if let Some(plic) = helper() {
        PLIC.init(Some(SpinLock::new(plic)));
    } else {
        not_found();
    }
}

pub fn not_found() {
    PLIC.init(None);
}

pub fn enable_interrupt_for_hart(hart_id: usize) {
    if let Some(ref plic) = *PLIC {
        let mut plic = plic.lock();
        plic.set_hart_threshold(hart_id, 0);
    }
}

pub fn enable_irq_for_all_harts(irq: u32) {
    if let Some(ref plic) = *PLIC {
        let mut plic = plic.lock();
        plic.enable_irq_for_all_harts(irq);
        plic.set_irq_priority(irq, 1);
    }
}

pub fn claim_irq(hart_id: usize) -> Option<u32> {
    if let Some(ref plic) = *PLIC {
        let mut plic = plic.lock();
        plic.claim_irq(hart_id)
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
