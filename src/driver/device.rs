use fdt::node::FdtNode;

pub enum DeviceType {
    Block,
    Char,
    Rtc,
}

pub struct Device<'a> {
    // mmio_base: usize,
    // mmio_size: usize,
    // name: &'a str,
    // compatible: &'a str,
    // interrupt_number: Option<u32>,
    fdt_node: &'a FdtNode<'a, 'a>,
}

impl<'a> Device<'a> {
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

    pub fn new(fdt_node: &'a FdtNode) -> Device<'a> {
        Device {
            fdt_node,
        }
    }

    pub fn mmio(&self) -> Option<(usize, usize)> {
        let mut reg_prop = self.fdt_node.reg()?;
        let reg = reg_prop.next()?;
        Some((reg.starting_address as usize, reg.size? as usize))
    }

    pub fn name(&self) -> &'a str {
        self.fdt_node.name
    }

    pub fn fdt_node(&self) -> &'a FdtNode<'a, 'a> {
        self.fdt_node
    }

    pub fn match_compatible(&self, mathches: &[&str]) -> Option<usize> {
        if let Some(compatible) = self.fdt_node.compatible() {
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
        self.fdt_node.property("interrupts").and_then(|p| p.as_usize()).map(|v| v as u32)
    }
}

impl<'a> core::fmt::Debug for Device<'a> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Device")
            .field("name", &self.name())
            .field("mmio", &self.mmio())
            .field("interrupt_number", &self.interrupt_number())
            .finish()
    }
}
