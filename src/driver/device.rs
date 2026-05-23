use fdt::Fdt;
use fdt::node::FdtNode;
use virtio_drivers::transport::pci::bus::{
    BarInfo, Cam, CapabilityIterator, DeviceFunction, DeviceFunctionInfo, PciRoot,
};

use crate::arch;

pub enum DeviceType {
    Block,
    Char,
    Pci,
    Pmu,
    Rtc,
    Net,
}

pub struct MMIODevice<'b, 'a: 'b> {
    // mmio_base: usize,
    // mmio_size: usize,
    // name: &'a str,
    // compatible: &'a str,
    // interrupt_number: Option<u32>,
    fdt: &'b Fdt<'a>,
    fdt_node: FdtNode<'b, 'a>,
}

pub struct PCIDevice<'a> {
    root: &'a mut PciRoot,
    ecam_kaddr: usize,
    cam: Cam,
    device_function: DeviceFunction,
    info: DeviceFunctionInfo,
    bars: [Option<BarInfo>; 6],
    interrupt: Option<PciInterrupt>,
}

pub enum Device<'b, 'a: 'b> {
    MMIO(MMIODevice<'b, 'a>),
    PCI(PCIDevice<'b>),
}

#[derive(Copy, Clone, Debug)]
pub enum PciInterrupt {
    Intx(u32),
    Msi(u32),
    Msix { irq: u32, vector: u16 },
}

impl PciInterrupt {
    pub fn irq(self) -> u32 {
        match self {
            Self::Intx(irq) | Self::Msi(irq) | Self::Msix { irq, .. } => irq,
        }
    }

    pub fn is_message_signaled(self) -> bool {
        matches!(self, Self::Msi(_) | Self::Msix { .. })
    }

    pub fn msix_vector(self) -> Option<u16> {
        match self {
            Self::Msix { vector, .. } => Some(vector),
            _ => None,
        }
    }
}

impl<'b, 'a: 'b> Device<'b, 'a> {
    // pub fn new(mmio_base: usize, mmio_size: usize, name: &'a str, compatible: &'a str, interrupt_number: Option<u32>, fdt_node: &'a FdtNode) -> Device<'a> {
    //     Device {
    //         // mmio_base,
    //         // mmio_size,
    //         // name,
    //         // compatible,
    //         // interrupt_number,
    //         fdt_node,
    //     }
    // }

    pub fn new(fdt: &'b Fdt<'a>, fdt_node: FdtNode<'b, 'a>) -> Self {
        Self::MMIO(MMIODevice { fdt, fdt_node })
    }

    pub fn mmio(&self) -> Option<(usize, usize)> {
        let Self::MMIO(device) = self else {
            return None;
        };
        let mut reg_prop = device.fdt_node.reg()?;
        let reg = reg_prop.next()?;
        Some((reg.starting_address as usize, reg.size? as usize))
    }

    pub fn name(&self) -> &'a str {
        match self {
            Self::MMIO(device) => device.fdt_node.name,
            Self::PCI(_) => "pci",
        }
    }

    pub fn fdt_node(&self) -> FdtNode<'b, 'a> {
        match self {
            Self::MMIO(device) => device.fdt_node,
            Self::PCI(_) => panic!("PCI device has no FDT node"),
        }
    }

    pub fn find_phandle(&self, phandle: u32) -> Option<FdtNode<'b, 'a>> {
        let Self::MMIO(device) = self else {
            return None;
        };
        device.fdt.find_phandle(phandle)
    }

    pub fn match_compatible(&self, mathches: &[&str]) -> Option<usize> {
        let Self::MMIO(device) = self else {
            return None;
        };
        if let Some(compatible) = device.fdt_node.compatible() {
            for (i, comp) in compatible.all().enumerate() {
                if mathches.contains(&comp) {
                    return Some(i);
                }
            }
            None
        } else {
            None
        }
    }

    pub fn interrupt_number(&self) -> Option<u32> {
        let Self::MMIO(device) = self else {
            return self.pci().and_then(|device| device.interrupt_number());
        };
        // `interrupts` encodes one or more 32-bit BE cells; the first cell is
        // always the hwirq number. Additional cells (e.g. trigger mode on
        // LoongArch) are consumed by the interrupt-parent per its own
        // #interrupt-cells. Using `as_usize()` on a multi-cell value returns
        // the *last* cell on this fdt crate, so read the raw first 4 bytes.
        let prop = device.fdt_node.property("interrupts")?;
        let bytes = prop.value;
        if bytes.len() < 4 {
            return None;
        }
        Some(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    pub fn pci_mut(&mut self) -> Option<&mut PCIDevice<'b>> {
        match self {
            Self::PCI(device) => Some(device),
            Self::MMIO(_) => None,
        }
    }

    pub fn is_pci(&self) -> bool {
        matches!(self, Self::PCI(_))
    }

    fn pci(&self) -> Option<&PCIDevice<'b>> {
        match self {
            Self::PCI(device) => Some(device),
            Self::MMIO(_) => None,
        }
    }
}

impl<'a> Device<'a, 'a> {
    pub fn new_pci(
        root: &'a mut PciRoot,
        ecam_kaddr: usize,
        cam: Cam,
        device_function: DeviceFunction,
        info: DeviceFunctionInfo,
        bars: [Option<BarInfo>; 6],
        interrupt: Option<PciInterrupt>,
    ) -> Self {
        Self::PCI(PCIDevice {
            root,
            ecam_kaddr,
            cam,
            device_function,
            info,
            bars,
            interrupt,
        })
    }
}

impl PCIDevice<'_> {
    pub fn root(&mut self) -> &mut PciRoot {
        self.root
    }

    pub fn device_function(&self) -> DeviceFunction {
        self.device_function
    }

    pub fn info(&self) -> &DeviceFunctionInfo {
        &self.info
    }

    pub fn interrupt_number(&self) -> Option<u32> {
        self.interrupt.map(PciInterrupt::irq)
    }

    pub fn msix_vector(&self) -> Option<u16> {
        self.interrupt.and_then(PciInterrupt::msix_vector)
    }

    pub(crate) fn capabilities(&self) -> CapabilityIterator<'_> {
        self.root.capabilities(self.device_function)
    }

    pub(crate) fn bar_address_size(&self, bar_index: u8) -> Option<(u64, u32)> {
        self.bars[bar_index as usize].as_ref()?.memory_address_size()
    }

    pub(crate) fn config_read_u32(&self, register_offset: u16) -> u32 {
        let ptr = (self.ecam_kaddr + config_offset(self.cam, self.device_function, register_offset)) as *const u32;
        unsafe { arch::read_volatile(ptr) }
    }
}

fn config_offset(cam: Cam, df: DeviceFunction, register_offset: u16) -> usize {
    let bdf = ((df.bus as usize) << 8) | ((df.device as usize) << 3) | df.function as usize;
    let config_off = match cam {
        Cam::MmioCam => bdf << 8,
        Cam::Ecam => bdf << 12,
    };
    config_off + register_offset as usize
}

impl core::fmt::Debug for Device<'_, '_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::MMIO(device) => f
                .debug_struct("MMIODevice")
                .field("name", &device.fdt_node.name)
                .field("mmio", &self.mmio())
                .field("interrupt_number", &self.interrupt_number())
                .finish(),
            Self::PCI(device) => f
                .debug_struct("PCIDevice")
                .field("device_function", &device.device_function)
                .field("info", &device.info)
                .field("interrupt", &device.interrupt)
                .finish(),
        }
    }
}
