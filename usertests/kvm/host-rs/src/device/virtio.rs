use std::{mem, ptr};
use num_enum::TryFromPrimitive;
use tokio::task::JoinHandle;

use crate::device::bus::{Bus, BusRef, MmioDevice};
use crate::dtb::{DtbBuilder, DtbConfig, dtb_node_name, dtb_reg_cells};

pub const MMIO_LENGTH: usize = 0x1000;

const MMIO_CONFIG_OFFSET: usize = 0x100;
const MMIO_MAGIC: u32 = 0x7472_6976;
const MMIO_VERSION: u32 = 2;
const MMIO_VENDOR: u32 = 0x554d_4551;

pub struct VirtioCommon {
    device_id: VirtioDeviceId,
    queue_num_max: u32,
    queues: Box<[VirtQueue]>,
    queue_select: usize,
    device_features_select: u32,
    driver_features_select: u32,
    driver_features: [u32; 2],
    status: u32,
    interrupt_status: u32,
    notify_pending: Box<[bool]>,
}

pub trait BackendOps: Send {
    fn device_id(&self) -> VirtioDeviceId;
    fn queue_count(&self) -> usize;
    fn queue_num_max(&self) -> u32;
    fn config(&self) -> &[u8];
    fn handle_request(&mut self, bus: &Bus, request: &VirtioRequest) -> VirtioRequestHandling;
    fn pop_completion(&mut self, _bus: &Bus) -> Option<VirtioCompletion> {
        None
    }
    fn spawn_tasks(&self, _bus: BusRef) -> Vec<JoinHandle<()>> {
        Vec::new()
    }
}

pub struct VirtioBackend<T: BackendOps> {
    common: VirtioCommon,
    backend: T,
}

pub struct VirtioRequest {
    pub queue_idx: usize,
    pub desc_idx: u16,
    pub buffers: Vec<RequestBuffer>,
}

pub enum VirtioRequestHandling {
    Complete(u32),
    Pending,
}

pub struct VirtioCompletion {
    pub queue_idx: usize,
    pub desc_idx: u16,
    pub len: u32,
}

#[derive(Clone, Copy)]
pub struct RequestBuffer {
    pub addr: u64,
    pub len: u32,
    pub write: bool,
}

#[repr(u32)]
#[derive(Clone, Copy)]
pub enum VirtioDeviceId {
    Block = 2,
}

#[derive(Clone, Copy, Default)]
struct VirtQueue {
    desc_table: u64,
    avail_ring: u64,
    used_ring: u64,
    queue_num: u32,
    ready: bool,
    last_avail_idx: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Descriptor {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

#[repr(usize)]
#[derive(Clone, Copy, TryFromPrimitive)]
enum VirtioMmioRegister {
    MagicValue = 0x000,
    Version = 0x004,
    DeviceId = 0x008,
    VendorId = 0x00c,
    DeviceFeatures = 0x010,
    DeviceFeaturesSel = 0x014,
    DriverFeatures = 0x020,
    DriverFeaturesSel = 0x024,
    QueueSel = 0x030,
    QueueNumMax = 0x034,
    QueueNum = 0x038,
    QueueReady = 0x044,
    QueueNotify = 0x050,
    InterruptStatus = 0x060,
    InterruptAck = 0x064,
    Status = 0x070,
    QueueDescLow = 0x080,
    QueueDescHigh = 0x084,
    QueueDriverLow = 0x090,
    QueueDriverHigh = 0x094,
    QueueDeviceLow = 0x0a0,
    QueueDeviceHigh = 0x0a4,
}

#[repr(u32)]
#[derive(Clone, Copy)]
enum VirtioFeature {
    Version1 = 32,
}

#[repr(u32)]
#[derive(Clone, Copy)]
enum VirtioStatusBit {
    FeaturesOk = 8,
}

#[repr(u32)]
#[derive(Clone, Copy)]
enum VirtioInterruptStatus {
    UsedBuffer = 1,
}

#[repr(u16)]
#[derive(Clone, Copy)]
enum DescriptorFlag {
    Next = 1,
    Write = 2,
    Indirect = 4,
}

#[repr(u16)]
#[derive(Clone, Copy)]
enum VirtqUsedFlag {
    NoNotify = 1,
}

#[derive(Clone, Copy)]
enum QueueAddressField {
    Desc,
    Avail,
    Used,
}

impl VirtioCommon {
    pub fn new(device_id: VirtioDeviceId, queue_count: usize, queue_num_max: u32) -> Self {
        Self {
            device_id,
            queue_num_max,
            queues: vec![VirtQueue::default(); queue_count].into_boxed_slice(),
            queue_select: 0,
            device_features_select: 0,
            driver_features_select: 0,
            driver_features: [0; 2],
            status: 0,
            interrupt_status: 0,
            notify_pending: vec![false; queue_count].into_boxed_slice(),
        }
    }

    pub fn read(&self, offset: usize, width: usize, config: &[u8]) -> Option<u64> {
        if offset >= MMIO_CONFIG_OFFSET {
            return self.read_config(offset - MMIO_CONFIG_OFFSET, width, config);
        }
        if width != mem::size_of::<u32>() {
            return None;
        }
        let value = match VirtioMmioRegister::try_from(offset).ok()? {
            VirtioMmioRegister::MagicValue => MMIO_MAGIC,
            VirtioMmioRegister::Version => MMIO_VERSION,
            VirtioMmioRegister::DeviceId => self.device_id as u32,
            VirtioMmioRegister::VendorId => MMIO_VENDOR,
            VirtioMmioRegister::DeviceFeatures => self.read_device_features(),
            VirtioMmioRegister::DeviceFeaturesSel => self.device_features_select,
            VirtioMmioRegister::DriverFeatures => 0,
            VirtioMmioRegister::DriverFeaturesSel => self.driver_features_select,
            VirtioMmioRegister::QueueSel => self.queue_select as u32,
            VirtioMmioRegister::QueueNumMax => {
                if self.selected_queue().is_some() {
                    self.queue_num_max
                } else {
                    0
                }
            }
            VirtioMmioRegister::QueueNum => self.selected_queue().map_or(0, |queue| queue.queue_num),
            VirtioMmioRegister::QueueReady => self.selected_queue().map_or(0, |queue| u32::from(queue.ready)),
            VirtioMmioRegister::QueueNotify => 0,
            VirtioMmioRegister::InterruptStatus => self.interrupt_status,
            VirtioMmioRegister::InterruptAck => 0,
            VirtioMmioRegister::Status => self.status,
            VirtioMmioRegister::QueueDescLow => self.selected_queue().map_or(0, |queue| queue.desc_table as u32),
            VirtioMmioRegister::QueueDescHigh => {
                self.selected_queue().map_or(0, |queue| (queue.desc_table >> 32) as u32)
            }
            VirtioMmioRegister::QueueDriverLow => self.selected_queue().map_or(0, |queue| queue.avail_ring as u32),
            VirtioMmioRegister::QueueDriverHigh => {
                self.selected_queue().map_or(0, |queue| (queue.avail_ring >> 32) as u32)
            }
            VirtioMmioRegister::QueueDeviceLow => self.selected_queue().map_or(0, |queue| queue.used_ring as u32),
            VirtioMmioRegister::QueueDeviceHigh => {
                self.selected_queue().map_or(0, |queue| (queue.used_ring >> 32) as u32)
            }
        };
        Some(u64::from(value))
    }

    pub fn write(&mut self, offset: usize, width: usize, value: u64, config_len: usize) -> bool {
        if offset >= MMIO_CONFIG_OFFSET {
            return offset
                .checked_add(width)
                .is_some_and(|end| end <= MMIO_CONFIG_OFFSET + config_len);
        }
        if width != mem::size_of::<u32>() {
            return false;
        }
        let data = value as u32;
        let Ok(register) = VirtioMmioRegister::try_from(offset) else {
            return false;
        };
        match register {
            VirtioMmioRegister::DeviceFeaturesSel => self.device_features_select = data,
            VirtioMmioRegister::DriverFeatures => self.write_driver_features(data),
            VirtioMmioRegister::DriverFeaturesSel => self.driver_features_select = data,
            VirtioMmioRegister::QueueSel => {
                if (data as usize) < self.queues.len() {
                    self.queue_select = data as usize;
                }
            }
            VirtioMmioRegister::QueueNum => {
                let queue_num_max = self.queue_num_max;
                if let Some(queue) = self.selected_queue_mut() {
                    queue.queue_num = data.min(queue_num_max);
                }
            }
            VirtioMmioRegister::QueueReady => {
                if let Some(queue) = self.selected_queue_mut() {
                    queue.ready = data == 1;
                }
            }
            VirtioMmioRegister::QueueNotify => {
                if let Some(pending) = self.notify_pending.get_mut(data as usize) {
                    *pending = true;
                }
            }
            VirtioMmioRegister::InterruptAck => self.interrupt_status &= !data,
            VirtioMmioRegister::Status => self.write_status(data),
            VirtioMmioRegister::QueueDescLow => self.set_selected_queue_addr_lower(data, QueueAddressField::Desc),
            VirtioMmioRegister::QueueDescHigh => self.set_selected_queue_addr_upper(data, QueueAddressField::Desc),
            VirtioMmioRegister::QueueDriverLow => self.set_selected_queue_addr_lower(data, QueueAddressField::Avail),
            VirtioMmioRegister::QueueDriverHigh => self.set_selected_queue_addr_upper(data, QueueAddressField::Avail),
            VirtioMmioRegister::QueueDeviceLow => self.set_selected_queue_addr_lower(data, QueueAddressField::Used),
            VirtioMmioRegister::QueueDeviceHigh => self.set_selected_queue_addr_upper(data, QueueAddressField::Used),
            VirtioMmioRegister::MagicValue
            | VirtioMmioRegister::Version
            | VirtioMmioRegister::DeviceId
            | VirtioMmioRegister::VendorId
            | VirtioMmioRegister::DeviceFeatures
            | VirtioMmioRegister::QueueNumMax
            | VirtioMmioRegister::InterruptStatus => return false,
        }
        true
    }

    pub fn pop_avail_request(&mut self, bus: &Bus) -> Option<VirtioRequest> {
        for queue_idx in 0..self.queues.len() {
            if !self.notify_pending[queue_idx] {
                continue;
            }
            if let Some(request) = self.pop_avail_request_from_queue(bus, queue_idx) {
                return Some(request);
            }
            self.notify_pending[queue_idx] = false;
        }
        None
    }

    pub fn push_used_completion(&mut self, bus: &Bus, queue_idx: usize, desc_idx: u16, len: u32) {
        let Some(queue) = self.queues.get(queue_idx).copied() else {
            return;
        };
        if queue.queue_num == 0 {
            return;
        }
        let Some(flags) = read_guest_u16(bus, queue.used_ring) else {
            return;
        };
        let Some(used_idx_addr) = queue.used_ring.checked_add(2) else {
            return;
        };
        let Some(used_idx) = read_guest_u16(bus, used_idx_addr) else {
            return;
        };
        let ring_idx = used_idx % queue.queue_num as u16;
        let Some(ring_offset) = u64::from(ring_idx)
            .checked_mul(8)
            .and_then(|offset| 4u64.checked_add(offset))
        else {
            return;
        };
        let Some(elem_addr) = queue.used_ring.checked_add(ring_offset) else {
            return;
        };
        let Some(len_addr) = elem_addr.checked_add(4) else {
            return;
        };
        if write_guest_u32(bus, elem_addr, u32::from(desc_idx)).is_none()
            || write_guest_u32(bus, len_addr, len).is_none()
            || write_guest_u16(bus, used_idx_addr, used_idx.wrapping_add(1)).is_none()
        {
            return;
        }
        if (flags & VirtqUsedFlag::NoNotify.mask()) == 0 {
            self.interrupt_status |= VirtioInterruptStatus::UsedBuffer.mask();
        }
    }

    pub fn interrupt_pending(&self) -> bool {
        (self.interrupt_status & VirtioInterruptStatus::UsedBuffer.mask()) != 0
    }

    pub fn config_dtb(&self, builder: &mut DtbBuilder, config: &DtbConfig, addr: usize, len: usize, id: u32) {
        builder.begin_node(&dtb_node_name("virtio_mmio", addr));
        builder.prop_string("compatible", "virtio,mmio");
        builder.prop_cells("reg", &dtb_reg_cells(addr, len));
        if id != 0 {
            builder.prop_u32("interrupt-parent", config.plic_phandle);
            builder.prop_u32("interrupts", id);
        }
        builder.end_node();
    }

    fn reset(&mut self) {
        for queue in self.queues.iter_mut() {
            *queue = VirtQueue::default();
        }
        for pending in self.notify_pending.iter_mut() {
            *pending = false;
        }
        self.queue_select = 0;
        self.device_features_select = 0;
        self.driver_features_select = 0;
        self.driver_features = [0; 2];
        self.status = 0;
        self.interrupt_status = 0;
    }

    fn read_config(&self, offset: usize, width: usize, config: &[u8]) -> Option<u64> {
        let end = offset.checked_add(width)?;
        if end > config.len() {
            return None;
        }
        let mut value = 0u64;
        for (i, byte) in config[offset..end].iter().enumerate() {
            value |= u64::from(*byte) << (i * 8);
        }
        Some(value)
    }

    fn read_device_features(&self) -> u32 {
        if self.device_features_select == VirtioFeature::Version1.select() {
            VirtioFeature::Version1.select_mask()
        } else {
            0
        }
    }

    fn write_driver_features(&mut self, value: u32) {
        let select = self.driver_features_select as usize;
        if let Some(features) = self.driver_features.get_mut(select) {
            *features = value;
        }
    }

    fn write_status(&mut self, value: u32) {
        if value == 0 {
            self.reset();
            return;
        }
        self.status = value;
        if (self.status & VirtioStatusBit::FeaturesOk.mask()) != 0 && !self.driver_features_supported() {
            self.status &= !VirtioStatusBit::FeaturesOk.mask();
        }
    }

    fn driver_features_supported(&self) -> bool {
        let supported = [0, VirtioFeature::Version1.select_mask()];
        self.driver_features
            .iter()
            .zip(supported.iter())
            .all(|(driver, supported)| (driver & !supported) == 0)
    }

    fn selected_queue(&self) -> Option<&VirtQueue> {
        self.queues.get(self.queue_select)
    }

    fn selected_queue_mut(&mut self) -> Option<&mut VirtQueue> {
        self.queues.get_mut(self.queue_select)
    }

    fn set_selected_queue_addr_lower(&mut self, value: u32, field: QueueAddressField) {
        if let Some(queue) = self.selected_queue_mut() {
            let addr = field.get_mut(queue);
            *addr = (*addr & !0xffff_ffff) | u64::from(value);
        }
    }

    fn set_selected_queue_addr_upper(&mut self, value: u32, field: QueueAddressField) {
        if let Some(queue) = self.selected_queue_mut() {
            let addr = field.get_mut(queue);
            *addr = (*addr & 0xffff_ffff) | (u64::from(value) << 32);
        }
    }

    fn pop_avail_request_from_queue(&mut self, bus: &Bus, queue_idx: usize) -> Option<VirtioRequest> {
        let queue = self.queues.get(queue_idx).copied()?;
        if !queue.ready || queue.queue_num == 0 || queue.queue_num > self.queue_num_max {
            return None;
        }
        let queue_num = queue.queue_num as u16;
        let avail_idx = read_guest_u16(bus, queue.avail_ring.checked_add(2)?)?;
        if queue.last_avail_idx == avail_idx {
            return None;
        }

        let ring_offset = 4u64.checked_add(u64::from(queue.last_avail_idx % queue_num).checked_mul(2)?)?;
        let desc_idx = read_guest_u16(bus, queue.avail_ring.checked_add(ring_offset)?)?;
        let buffers = self.read_descriptor_chain(bus, queue, desc_idx).unwrap_or_default();
        self.queues[queue_idx].last_avail_idx = queue.last_avail_idx.wrapping_add(1);
        Some(VirtioRequest {
            queue_idx,
            desc_idx,
            buffers,
        })
    }

    fn read_descriptor_chain(&self, bus: &Bus, queue: VirtQueue, desc_idx: u16) -> Option<Vec<RequestBuffer>> {
        if queue.queue_num == 0 || u32::from(desc_idx) >= queue.queue_num {
            return None;
        }
        let mut buffers = Vec::new();
        let mut next_idx = desc_idx;
        for _ in 0..queue.queue_num {
            let descriptor = read_descriptor(bus, queue, next_idx)?;
            if descriptor.has_flag(DescriptorFlag::Indirect) {
                return None;
            }
            buffers.push(RequestBuffer {
                addr: descriptor.addr,
                len: descriptor.len,
                write: descriptor.has_flag(DescriptorFlag::Write),
            });
            if !descriptor.has_flag(DescriptorFlag::Next) {
                return Some(buffers);
            }
            next_idx = descriptor.next;
            if u32::from(next_idx) >= queue.queue_num {
                return None;
            }
        }
        None
    }
}

impl<T: BackendOps> VirtioBackend<T> {
    pub const LENGTH: usize = MMIO_LENGTH;

    pub fn new(backend: T) -> Self {
        Self {
            common: VirtioCommon::new(backend.device_id(), backend.queue_count(), backend.queue_num_max()),
            backend,
        }
    }

    fn handle_queue(&mut self, bus: &Bus) {
        self.push_pending_completions(bus);
        while let Some(request) = self.common.pop_avail_request(bus) {
            match self.backend.handle_request(bus, &request) {
                VirtioRequestHandling::Complete(len) => {
                    self.common
                        .push_used_completion(bus, request.queue_idx, request.desc_idx, len);
                }
                VirtioRequestHandling::Pending => {}
            }
        }
        self.push_pending_completions(bus);
    }

    fn push_pending_completions(&mut self, bus: &Bus) {
        while let Some(completion) = self.backend.pop_completion(bus) {
            self.common
                .push_used_completion(bus, completion.queue_idx, completion.desc_idx, completion.len);
        }
    }
}

impl<T: BackendOps> MmioDevice for VirtioBackend<T> {
    fn read(&mut self, offset: usize, width: usize) -> Option<u64> {
        self.common.read(offset, width, self.backend.config())
    }

    fn write(&mut self, offset: usize, width: usize, value: u64) -> bool {
        self.common.write(offset, width, value, self.backend.config().len())
    }

    fn update(&mut self, bus: &Bus) {
        self.handle_queue(bus);
    }

    fn spawn_tasks(&self, bus: BusRef, _guest_addr: usize, _length: usize, _id: u32) -> Vec<JoinHandle<()>> {
        self.backend.spawn_tasks(bus)
    }

    fn interrupt_pending(&self) -> bool {
        self.common.interrupt_pending()
    }

    fn config_dtb(&self, builder: &mut DtbBuilder, config: &DtbConfig, addr: usize, len: usize, id: u32) {
        self.common.config_dtb(builder, config, addr, len, id);
    }
}

impl QueueAddressField {
    fn get_mut(self, queue: &mut VirtQueue) -> &mut u64 {
        match self {
            QueueAddressField::Desc => &mut queue.desc_table,
            QueueAddressField::Avail => &mut queue.avail_ring,
            QueueAddressField::Used => &mut queue.used_ring,
        }
    }
}

impl VirtioFeature {
    fn select(self) -> u32 {
        (self as u32) / 32
    }

    fn select_mask(self) -> u32 {
        1u32 << ((self as u32) % 32)
    }
}

impl VirtioStatusBit {
    fn mask(self) -> u32 {
        self as u32
    }
}

impl VirtioInterruptStatus {
    fn mask(self) -> u32 {
        self as u32
    }
}

impl VirtqUsedFlag {
    fn mask(self) -> u16 {
        self as u16
    }
}

impl Descriptor {
    fn has_flag(self, flag: DescriptorFlag) -> bool {
        (self.flags & flag.mask()) != 0
    }
}

impl DescriptorFlag {
    fn mask(self) -> u16 {
        self as u16
    }
}

fn read_descriptor(bus: &Bus, queue: VirtQueue, idx: u16) -> Option<Descriptor> {
    let offset = u64::from(idx).checked_mul(mem::size_of::<Descriptor>() as u64)?;
    let addr = queue.desc_table.checked_add(offset)?;
    Some(Descriptor {
        addr: read_guest_u64(bus, addr)?,
        len: read_guest_u32(bus, addr.checked_add(8)?)?,
        flags: read_guest_u16(bus, addr.checked_add(12)?)?,
        next: read_guest_u16(bus, addr.checked_add(14)?)?,
    })
}

pub fn translate_guest_slice(bus: &Bus, addr: u64, len: u32) -> Option<&[u8]> {
    let len = usize::try_from(len).ok()?;
    let ptr = bus.translate(usize::try_from(addr).ok()?, len)?;
    Some(unsafe { std::slice::from_raw_parts(ptr.cast_const(), len) })
}

pub fn translate_guest_slice_mut(bus: &Bus, addr: u64, len: u32) -> Option<&mut [u8]> {
    let len = usize::try_from(len).ok()?;
    let ptr = bus.translate(usize::try_from(addr).ok()?, len)?;
    Some(unsafe { std::slice::from_raw_parts_mut(ptr, len) })
}

pub fn read_guest_u32(bus: &Bus, addr: u64) -> Option<u32> {
    let ptr = bus.translate(usize::try_from(addr).ok()?, mem::size_of::<u32>())?;
    Some(u32::from_le(unsafe { ptr::read_unaligned(ptr.cast::<u32>()) }))
}

pub fn read_guest_u64(bus: &Bus, addr: u64) -> Option<u64> {
    let ptr = bus.translate(usize::try_from(addr).ok()?, mem::size_of::<u64>())?;
    Some(u64::from_le(unsafe { ptr::read_unaligned(ptr.cast::<u64>()) }))
}

pub fn write_guest_u8(bus: &Bus, addr: u64, value: u8) -> Option<()> {
    let ptr = bus.translate(usize::try_from(addr).ok()?, mem::size_of::<u8>())?;
    unsafe { ptr::write_unaligned(ptr, value) };
    Some(())
}

fn read_guest_u16(bus: &Bus, addr: u64) -> Option<u16> {
    let ptr = bus.translate(usize::try_from(addr).ok()?, mem::size_of::<u16>())?;
    Some(u16::from_le(unsafe { ptr::read_unaligned(ptr.cast::<u16>()) }))
}

fn write_guest_u16(bus: &Bus, addr: u64, value: u16) -> Option<()> {
    let ptr = bus.translate(usize::try_from(addr).ok()?, mem::size_of::<u16>())?;
    unsafe { ptr::write_unaligned(ptr.cast::<u16>(), value.to_le()) };
    Some(())
}

fn write_guest_u32(bus: &Bus, addr: u64, value: u32) -> Option<()> {
    let ptr = bus.translate(usize::try_from(addr).ok()?, mem::size_of::<u32>())?;
    unsafe { ptr::write_unaligned(ptr.cast::<u32>(), value.to_le()) };
    Some(())
}
