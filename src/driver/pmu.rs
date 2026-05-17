use alloc::string::String;
use alloc::sync::Arc;
use core::mem::size_of;

use fdt::node::FdtNode;

use crate::arch;
use crate::driver::chosen::kpmu::{self, KPMU};
use crate::driver::matcher::DriverMatcher;
use crate::driver::{Device, DeviceType, DriverOps};

#[derive(Clone, Copy)]
enum RegWidth {
    U8,
    U16,
    U32,
}

impl RegWidth {
    fn from_bytes(bytes: usize) -> Option<Self> {
        match bytes {
            1 => Some(Self::U8),
            2 => Some(Self::U16),
            4 => Some(Self::U32),
            _ => None,
        }
    }

    fn bytes(self) -> usize {
        match self {
            Self::U8 => 1,
            Self::U16 => 2,
            Self::U32 => 4,
        }
    }
}

pub struct SysconPoweroff {
    base: usize,
    offset: usize,
    value: u32,
    width: RegWidth,
    name: String,
}

impl SysconPoweroff {
    fn new(base: usize, offset: usize, value: u32, width: RegWidth, name: String) -> Self {
        Self {
            base,
            offset,
            value,
            width,
            name,
        }
    }

    fn write_poweroff(&self) {
        let addr = self.base + self.offset;
        match self.width {
            RegWidth::U8 => unsafe {
                arch::write_volatile(addr as *mut u8, self.value as u8);
            },
            RegWidth::U16 => unsafe {
                arch::write_volatile(addr as *mut u16, self.value as u16);
            },
            RegWidth::U32 => unsafe {
                arch::write_volatile(addr as *mut u32, self.value);
            },
        }
    }
}

impl DriverOps for SysconPoweroff {
    fn name(&self) -> &str {
        "syscon-poweroff"
    }

    fn device_name(&self) -> String {
        self.name.clone()
    }

    fn device_type(&self) -> DeviceType {
        DeviceType::Pmu
    }
}

impl KPMU for SysconPoweroff {
    fn shutdown(&self) -> ! {
        self.write_poweroff();
        loop {}
    }
}

pub struct Matcher;

impl DriverMatcher for Matcher {
    fn try_match(&self, device: &Device) -> Option<Arc<dyn DriverOps>> {
        device.match_compatible(&["syscon-poweroff"])?;

        let phandle = read_property_u32(device.fdt_node(), "regmap")?;
        let offset = read_property_u32(device.fdt_node(), "offset")? as usize;
        let value = read_property_u32(device.fdt_node(), "value")?;

        let syscon = device.find_phandle(phandle)?;
        syscon.compatible()?.all().find(|c| *c == "syscon")?;

        let (mmio_base, mmio_size) = first_reg(syscon)?;
        let width = RegWidth::from_bytes(read_property_u32(syscon, "reg-io-width").unwrap_or(4) as usize)?;
        let end = offset.checked_add(width.bytes())?;
        if end > mmio_size {
            return None;
        }

        let base = arch::mmio_phys_to_kaddr(mmio_base, mmio_size);
        let driver = Arc::new(SysconPoweroff::new(base, offset, value, width, device.name().into()));
        kpmu::register(driver.clone());
        Some(driver)
    }
}

fn read_property_u32(node: FdtNode, name: &str) -> Option<u32> {
    let value = node.property(name)?.value;
    if value.len() < size_of::<u32>() {
        return None;
    }
    Some(u32::from_be_bytes(value[..size_of::<u32>()].try_into().ok()?))
}

fn first_reg(node: FdtNode) -> Option<(usize, usize)> {
    let mut regs = node.reg()?;
    let reg = regs.next()?;
    Some((reg.starting_address as usize, reg.size?))
}
