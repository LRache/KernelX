use std::mem;
use num_enum::TryFromPrimitive;

use crate::device::bus::{Bus, MmioDevice};
use crate::dtb::{DtbBuilder, DtbConfig, dtb_node_name, dtb_reg_cells};

const PLIC_SOURCE_COUNT: usize = 32;
const PLIC_CONTEXT_COUNT: usize = 32;
const PLIC_NUM_INTERRUPTS: u32 = 31;

#[derive(Clone, Copy, Default)]
struct InterruptSource {
    priority: u32,
    pending: bool,
    claimed: bool,
    enable: [bool; PLIC_CONTEXT_COUNT],
}

#[derive(Clone, Copy, Default)]
struct TargetContext {
    threshold: u32,
    claim: u32,
}

pub struct PlicDevice {
    interrupt_sources: [InterruptSource; PLIC_SOURCE_COUNT],
    target_contexts: [TargetContext; PLIC_CONTEXT_COUNT],
}

impl Default for PlicDevice {
    fn default() -> Self {
        Self {
            interrupt_sources: [InterruptSource::default(); PLIC_SOURCE_COUNT],
            target_contexts: [TargetContext::default(); PLIC_CONTEXT_COUNT],
        }
    }
}

#[derive(Clone, Copy)]
enum PlicAddressSpace {
    Priority,
    Pending,
    Enable,
    Context,
}

impl PlicAddressSpace {
    fn from_offset(offset: usize) -> Option<Self> {
        if Self::contains(offset, 0x000000, 0x001000) {
            Some(Self::Priority)
        } else if Self::contains(offset, 0x001000, 0x000080) {
            Some(Self::Pending)
        } else if Self::contains(offset, 0x002000, 0x1f0000) {
            Some(Self::Enable)
        } else if Self::contains(offset, 0x200000, 0x100000) {
            Some(Self::Context)
        } else {
            None
        }
    }

    fn contains(offset: usize, base: usize, size: usize) -> bool {
        offset >= base && offset < base + size
    }
}

#[repr(usize)]
#[derive(Clone, Copy, TryFromPrimitive)]
enum PlicContextRegister {
    Threshold = 0,
    Claim = 4,
}

impl PlicDevice {
    pub const LENGTH: usize = 0x0400_0000;

    fn is_u32_access(offset: usize, width: usize) -> bool {
        width == mem::size_of::<u32>() && (offset & (mem::size_of::<u32>() - 1)) == 0
    }

    fn scan_interrupt_sources(&mut self, bus: &Bus) {
        for region in bus.mmio_regions() {
            if region.id == 0 || region.id as usize >= PLIC_SOURCE_COUNT {
                continue;
            }
            if region
                .device
                .lock()
                .expect("mmio device lock poisoned")
                .interrupt_pending()
            {
                self.interrupt_sources[region.id as usize].pending = true;
            }
        }
    }

    fn refresh_context_claims(&mut self, bus: &Bus) {
        for context in 0..PLIC_CONTEXT_COUNT {
            if self.target_contexts[context].claim != 0 {
                continue;
            }
            let mut best_source = 0u32;
            let mut best_priority = self.target_contexts[context].threshold;
            for source in 1..PLIC_SOURCE_COUNT {
                let interrupt_source = self.interrupt_sources[source];
                if !interrupt_source.pending
                    || interrupt_source.claimed
                    || !interrupt_source.enable[context]
                    || interrupt_source.priority <= best_priority
                {
                    continue;
                }
                best_priority = interrupt_source.priority;
                best_source = source as u32;
            }
            if best_source == 0 {
                continue;
            }
            let source = best_source as usize;
            self.interrupt_sources[source].pending = false;
            self.interrupt_sources[source].claimed = true;
            self.target_contexts[context].claim = best_source;

            if let Some(region) = bus.mmio_regions().iter().find(|region| region.id == best_source) {
                region
                    .device
                    .lock()
                    .expect("mmio device lock poisoned")
                    .clear_interrupt();
            }
        }
    }
}

impl MmioDevice for PlicDevice {
    fn read(&mut self, offset: usize, width: usize) -> Option<u64> {
        if !Self::is_u32_access(offset, width) {
            return None;
        }
        match PlicAddressSpace::from_offset(offset) {
            Some(PlicAddressSpace::Priority) => {
                let source = offset / mem::size_of::<u32>();
                Some(self.interrupt_sources.get(source).map_or(0, |source| source.priority) as u64)
            }
            Some(PlicAddressSpace::Pending) => {
                let word = (offset - 0x001000) / mem::size_of::<u32>();
                let mut pending = 0u32;
                for bit in 0..32 {
                    let source = word * 32 + bit;
                    if source < PLIC_SOURCE_COUNT && self.interrupt_sources[source].pending {
                        pending |= 1u32 << bit;
                    }
                }
                Some(pending as u64)
            }
            Some(PlicAddressSpace::Enable) => {
                let context = (offset - 0x002000) / 0x80;
                let word = ((offset - 0x002000) % 0x80) / mem::size_of::<u32>();
                if context >= PLIC_CONTEXT_COUNT || word != 0 {
                    return Some(0);
                }
                let mut enabled = 0u32;
                for source in 0..PLIC_SOURCE_COUNT {
                    if self.interrupt_sources[source].enable[context] {
                        enabled |= 1u32 << source;
                    }
                }
                Some(enabled as u64)
            }
            Some(PlicAddressSpace::Context) => {
                let context = (offset - 0x200000) / 0x1000;
                let context_offset = (offset - 0x200000) % 0x1000;
                if context >= PLIC_CONTEXT_COUNT {
                    return Some(0);
                }
                match PlicContextRegister::try_from(context_offset) {
                    Ok(PlicContextRegister::Threshold) => Some(self.target_contexts[context].threshold as u64),
                    Ok(PlicContextRegister::Claim) => Some(self.target_contexts[context].claim as u64),
                    Err(_) => Some(0),
                }
            }
            None => None,
        }
    }

    fn write(&mut self, offset: usize, width: usize, value: u64) -> bool {
        if !Self::is_u32_access(offset, width) {
            return false;
        }
        let data = value as u32;
        match PlicAddressSpace::from_offset(offset) {
            Some(PlicAddressSpace::Priority) => {
                let source = offset / mem::size_of::<u32>();
                if source < PLIC_SOURCE_COUNT {
                    self.interrupt_sources[source].priority = data;
                }
                true
            }
            Some(PlicAddressSpace::Pending) => false,
            Some(PlicAddressSpace::Enable) => {
                let context = (offset - 0x002000) / 0x80;
                let word = ((offset - 0x002000) % 0x80) / mem::size_of::<u32>();
                if context >= PLIC_CONTEXT_COUNT || word != 0 {
                    return true;
                }
                for source in 0..PLIC_SOURCE_COUNT {
                    self.interrupt_sources[source].enable[context] = (data & (1u32 << source)) != 0;
                }
                self.interrupt_sources[0].enable[context] = false;
                true
            }
            Some(PlicAddressSpace::Context) => {
                let context = (offset - 0x200000) / 0x1000;
                let context_offset = (offset - 0x200000) % 0x1000;
                if context >= PLIC_CONTEXT_COUNT {
                    return false;
                }
                match PlicContextRegister::try_from(context_offset) {
                    Ok(PlicContextRegister::Threshold) => {
                        self.target_contexts[context].threshold = data;
                        true
                    }
                    Ok(PlicContextRegister::Claim) => {
                        if data == self.target_contexts[context].claim && (data as usize) < PLIC_SOURCE_COUNT {
                            self.interrupt_sources[data as usize].claimed = false;
                            self.target_contexts[context].claim = 0;
                        }
                        true
                    }
                    Err(_) => false,
                }
            }
            None => false,
        }
    }

    fn update(&mut self, bus: &Bus) {
        self.scan_interrupt_sources(bus);
        self.refresh_context_claims(bus);
    }

    fn interrupt_pending(&self) -> bool {
        self.target_contexts.iter().any(|context| context.claim != 0)
    }

    fn config_dtb(&self, builder: &mut DtbBuilder, config: &DtbConfig, addr: usize, len: usize, _id: u32) {
        builder.begin_node(&dtb_node_name("plic", addr));
        builder.prop_u32("phandle", config.plic_phandle);
        builder.prop_u32("riscv,ndev", PLIC_NUM_INTERRUPTS);
        builder.prop_cells("reg", &dtb_reg_cells(addr, len));
        builder.prop_cells(
            "interrupts-extended",
            &[config.cpu_intc_phandle, 11, config.cpu_intc_phandle, 9],
        );
        builder.prop_bool("interrupt-controller");
        builder.prop_string_list("compatible", &["sifive,plic-1.0.0", "riscv,plic0"]);
        builder.prop_u32("#address-cells", 0);
        builder.prop_u32("#interrupt-cells", 1);
        builder.end_node();
    }
}
