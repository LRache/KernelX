pub enum DeviceType {
    Block,
    Char,
    Network,
    Other,
}

#[derive(Debug)]
pub struct Device<'a> {
    pub mmio_base: usize,
    pub mmio_size: usize,
    pub name: &'a str,
    pub compatible: &'a str,
}

impl<'a> Device<'a> {
    pub fn new(mmio_base: usize, mmio_size: usize, name: &'a str, compatible: &'a str) -> Device<'a> {
        Device {
            mmio_base,
            mmio_size,
            name,
            compatible,
        }
    }

    pub fn mmio_base(&self) -> usize {
        self.mmio_base
    }

    pub fn mmio_size(&self) -> usize {
        self.mmio_size
    }

    pub fn name(&self) -> &'a str {
        self.name
    }

    pub fn compatible(&self) -> &'a str {
        self.compatible
    }
}
