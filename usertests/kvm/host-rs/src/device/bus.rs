use std::sync::{Arc, Mutex, TryLockError};
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio::time;

use crate::dtb::{DtbBuilder, DtbConfig};
use crate::kvm::Kvm;

pub type BusRef = Arc<Mutex<Bus>>;
pub type DeviceRef = Arc<Mutex<dyn MmioDevice + Send>>;

pub trait MmioDevice: Send {
    fn read(&mut self, offset: usize, width: usize) -> Option<u64>;
    fn write(&mut self, offset: usize, width: usize, value: u64) -> bool;
    fn update(&mut self, bus: &Bus) {
        let _ = bus;
    }
    fn spawn_tasks(&self, _bus: BusRef, _guest_addr: usize, _length: usize, _id: u32) -> Vec<JoinHandle<()>> {
        Vec::new()
    }
    fn interrupt_pending(&self) -> bool {
        false
    }
    fn clear_interrupt(&mut self) {}
    fn config_dtb(&self, builder: &mut DtbBuilder, config: &DtbConfig, addr: usize, len: usize, id: u32);
}

#[derive(Clone)]
struct Area {
    guest_addr: usize,
    length: usize,
    host_addr: *mut u8,
}

unsafe impl Send for Area {}

#[derive(Default)]
pub struct BusBuilder {
    mmio_regions: Vec<MmioRegion>,
}

#[derive(Clone)]
pub struct MmioRegion {
    pub guest_addr: usize,
    pub length: usize,
    pub id: u32,
    pub device: DeviceRef,
}

pub struct Bus {
    areas: Vec<Area>,
    mmio_regions: Box<[MmioRegion]>,
}

pub fn checked_range_end(start: usize, length: usize) -> Option<usize> {
    (length != 0).then(|| start.checked_add(length)).flatten()
}

fn ranges_overlap(left_start: usize, left_length: usize, right_start: usize, right_length: usize) -> bool {
    match (
        checked_range_end(left_start, left_length),
        checked_range_end(right_start, right_length),
    ) {
        (Some(left_end), Some(right_end)) => left_start < right_end && right_start < left_end,
        _ => true,
    }
}

impl BusBuilder {
    pub fn add_mmio_device(
        &mut self,
        guest_addr: usize,
        length: usize,
        device: DeviceRef,
        id: u32,
    ) -> Result<(), String> {
        let Some(end) = checked_range_end(guest_addr, length) else {
            return Err(format!(
                "bus mmio device has invalid range: addr=0x{guest_addr:x} length=0x{length:x}"
            ));
        };
        if self
            .mmio_regions
            .iter()
            .any(|region| ranges_overlap(guest_addr, length, region.guest_addr, region.length))
        {
            return Err(format!(
                "bus mmio device overlaps existing device: new=[0x{guest_addr:x},0x{end:x})"
            ));
        }
        self.mmio_regions.push(MmioRegion {
            guest_addr,
            length,
            id,
            device,
        });
        Ok(())
    }

    pub fn build(self) -> Bus {
        Bus {
            areas: Vec::new(),
            mmio_regions: self.mmio_regions.into_boxed_slice(),
        }
    }
}

impl Bus {
    pub fn mmio_regions(&self) -> &[MmioRegion] {
        &self.mmio_regions
    }

    pub fn map_area(&mut self, kvm: &Kvm, guest_addr: usize, length: usize) -> Result<*mut u8, String> {
        let Some(end) = checked_range_end(guest_addr, length) else {
            return Err(format!(
                "bus map area has invalid range: addr=0x{guest_addr:x} length=0x{length:x}"
            ));
        };
        if self
            .areas
            .iter()
            .any(|area| ranges_overlap(guest_addr, length, area.guest_addr, area.length))
        {
            return Err(format!(
                "bus map area overlaps existing range: new=[0x{guest_addr:x},0x{end:x})"
            ));
        }
        if self
            .mmio_regions
            .iter()
            .any(|region| ranges_overlap(guest_addr, length, region.guest_addr, region.length))
        {
            return Err(format!(
                "bus map area overlaps mmio device: addr=0x{guest_addr:x} length=0x{length:x}"
            ));
        }
        let host_addr = kvm.map_area_raw(guest_addr, length)?;
        self.areas.push(Area {
            guest_addr,
            length,
            host_addr,
        });
        Ok(host_addr)
    }

    pub fn translate(&self, guest_addr: usize, length: usize) -> Option<*mut u8> {
        let end = checked_range_end(guest_addr, length)?;
        self.areas.iter().find_map(|area| {
            let area_end = area.guest_addr.checked_add(area.length)?;
            if area.guest_addr <= guest_addr && end <= area_end {
                Some(unsafe { area.host_addr.add(guest_addr - area.guest_addr) })
            } else {
                None
            }
        })
    }

    pub fn read_mmio(&self, guest_addr: usize, width: usize) -> Option<u64> {
        let region = self.find_mmio_region(guest_addr, width)?;
        region
            .device
            .lock()
            .expect("mmio device lock poisoned")
            .read(guest_addr - region.guest_addr, width)
    }

    pub fn write_mmio(&self, guest_addr: usize, width: usize, value: u64) -> bool {
        let Some(region) = self.find_mmio_region(guest_addr, width) else {
            return false;
        };
        region
            .device
            .lock()
            .expect("mmio device lock poisoned")
            .write(guest_addr - region.guest_addr, width, value)
    }

    fn find_mmio_region(&self, guest_addr: usize, length: usize) -> Option<&MmioRegion> {
        let end = checked_range_end(guest_addr, length)?;
        self.mmio_regions.iter().find(|region| {
            let region_end = region.guest_addr + region.length;
            region.guest_addr <= guest_addr && end <= region_end
        })
    }

    pub fn external_interrupt_pending(&self) -> bool {
        self.mmio_regions.iter().any(|region| {
            region.id == 0
                && region
                    .device
                    .lock()
                    .expect("mmio device lock poisoned")
                    .interrupt_pending()
        })
    }

    pub fn update(&self) {
        for region in &self.mmio_regions {
            region.device.lock().expect("mmio device lock poisoned").update(self);
        }
    }

    pub fn spawn_runtime_tasks(bus: BusRef) -> Result<RuntimeTasks, String> {
        let regions: Vec<(usize, usize, u32, DeviceRef)> = {
            let bus = bus.lock().map_err(|_| "kvm bus lock poisoned".to_string())?;
            bus.mmio_regions
                .iter()
                .map(|region| (region.guest_addr, region.length, region.id, region.device.clone()))
                .collect()
        };

        let mut handles = Vec::with_capacity(regions.len());
        for (guest_addr, length, id, device) in regions {
            handles.extend(
                device
                    .lock()
                    .map_err(|_| "mmio device lock poisoned".to_string())?
                    .spawn_tasks(bus.clone(), guest_addr, length, id),
            );
        }
        Ok(RuntimeTasks { handles })
    }

    pub async fn notify(bus: &BusRef) -> Result<(), String> {
        loop {
            if Self::try_notify(bus)? {
                return Ok(());
            }
            time::sleep(Duration::from_millis(1)).await;
        }
    }

    fn try_notify(bus: &BusRef) -> Result<bool, String> {
        match bus.try_lock() {
            Ok(bus) => {
                bus.update();
                Ok(true)
            }
            Err(TryLockError::WouldBlock) => Ok(false),
            Err(TryLockError::Poisoned(_)) => Err("kvm bus lock poisoned".to_string()),
        }
    }

    pub fn build_dtb(&self, config: &DtbConfig) -> Vec<u8> {
        let mut builder = DtbBuilder::default();
        builder.config_dtb(config);
        for region in &self.mmio_regions {
            region.device.lock().expect("mmio device lock poisoned").config_dtb(
                &mut builder,
                config,
                region.guest_addr,
                region.length,
                region.id,
            );
        }
        builder.finish_dtb()
    }
}

pub struct RuntimeTasks {
    handles: Vec<JoinHandle<()>>,
}

impl Drop for RuntimeTasks {
    fn drop(&mut self) {
        for handle in &self.handles {
            handle.abort();
        }
    }
}
