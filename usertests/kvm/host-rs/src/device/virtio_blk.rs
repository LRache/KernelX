use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::mem;

use num_enum::TryFromPrimitive;

use crate::device::bus::{Bus, MmioDevice};
use crate::device::virtio::{
    self, RequestBuffer, VirtioCommon, VirtioDeviceId, read_guest_u32, read_guest_u64, translate_guest_slice,
    translate_guest_slice_mut, write_guest_u8,
};
use crate::dtb::{DtbBuilder, DtbConfig};

const SECTOR_SIZE: u64 = 512;
const QUEUE_COUNT: usize = 1;
const QUEUE_NUM_MAX: u32 = 128;

pub struct VirtioBlkDevice {
    file: File,
    capacity: u64,
    common: VirtioCommon,
}

#[repr(u32)]
#[derive(Clone, Copy, TryFromPrimitive)]
enum VirtioBlkRequestType {
    In = 0,
    Out = 1,
    Flush = 4,
    GetId = 8,
    GetLifetime = 10,
    Discard = 11,
    WriteZeroes = 13,
    SecureErase = 14,
}

#[repr(u8)]
#[derive(Clone, Copy)]
enum VirtioBlkStatus {
    Ok = 0,
    IoErr = 1,
    Unsupported = 2,
}

impl VirtioBlkDevice {
    pub const LENGTH: usize = virtio::MMIO_LENGTH;

    pub fn open(path: &str) -> Result<Self, String> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|err| format!("open virtio-blk image {path}: {err}"))?;
        let image_size = file
            .metadata()
            .map_err(|err| format!("stat virtio-blk image {path}: {err}"))?
            .len();
        if image_size < SECTOR_SIZE {
            return Err(format!("virtio-blk image {path} is smaller than one sector"));
        }
        Ok(Self {
            file,
            capacity: image_size / SECTOR_SIZE,
            common: VirtioCommon::new(VirtioDeviceId::Block, QUEUE_COUNT, QUEUE_NUM_MAX),
        })
    }

    fn config_bytes(&self) -> [u8; mem::size_of::<u64>()] {
        self.capacity.to_le_bytes()
    }

    fn handle_queue(&mut self, bus: &Bus) {
        while let Some(request) = self.common.pop_avail_request(bus) {
            let len = self.handle_blk_request(bus, &request.buffers).unwrap_or(0);
            self.common
                .push_used_completion(bus, request.queue_idx, request.desc_idx, len);
        }
    }

    fn handle_blk_request(&mut self, bus: &Bus, buffers: &[RequestBuffer]) -> Option<u32> {
        if buffers.len() < 2 {
            return None;
        }
        let header = buffers[0];
        let status = *buffers.last()?;
        if header.write || !status.write || header.len < 16 || status.len < 1 {
            return None;
        }
        let request_type = read_guest_u32(bus, header.addr)?;
        let sector = read_guest_u64(bus, header.addr.checked_add(8)?)?;
        let data_buffers = &buffers[1..buffers.len() - 1];

        let result = match VirtioBlkRequestType::try_from(request_type) {
            Ok(VirtioBlkRequestType::In) => self.blk_read(bus, sector, data_buffers),
            Ok(VirtioBlkRequestType::Out) => self.blk_write(bus, sector, data_buffers),
            Ok(VirtioBlkRequestType::Flush) => self.file.flush().map_err(|_| VirtioBlkStatus::IoErr).map(|_| 0),
            Ok(
                VirtioBlkRequestType::GetId
                | VirtioBlkRequestType::GetLifetime
                | VirtioBlkRequestType::Discard
                | VirtioBlkRequestType::WriteZeroes
                | VirtioBlkRequestType::SecureErase,
            )
            | Err(_) => Err(VirtioBlkStatus::Unsupported),
        };

        match result {
            Ok(data_len) => {
                write_guest_u8(bus, status.addr, VirtioBlkStatus::Ok.value())?;
                Some(data_len.checked_add(1)?)
            }
            Err(status_value) => {
                write_guest_u8(bus, status.addr, status_value.value())?;
                Some(1)
            }
        }
    }

    fn blk_read(&mut self, bus: &Bus, sector: u64, buffers: &[RequestBuffer]) -> Result<u32, VirtioBlkStatus> {
        let data_len = validate_data_buffers(buffers, true)?;
        self.validate_range(sector, data_len)?;
        self.file
            .seek(SeekFrom::Start(
                sector.checked_mul(SECTOR_SIZE).ok_or(VirtioBlkStatus::IoErr)?,
            ))
            .map_err(|_| VirtioBlkStatus::IoErr)?;
        for buffer in buffers {
            let slice = translate_guest_slice_mut(bus, buffer.addr, buffer.len).ok_or(VirtioBlkStatus::IoErr)?;
            self.file.read_exact(slice).map_err(|_| VirtioBlkStatus::IoErr)?;
        }
        Ok(data_len)
    }

    fn blk_write(&mut self, bus: &Bus, sector: u64, buffers: &[RequestBuffer]) -> Result<u32, VirtioBlkStatus> {
        let data_len = validate_data_buffers(buffers, false)?;
        self.validate_range(sector, data_len)?;
        self.file
            .seek(SeekFrom::Start(
                sector.checked_mul(SECTOR_SIZE).ok_or(VirtioBlkStatus::IoErr)?,
            ))
            .map_err(|_| VirtioBlkStatus::IoErr)?;
        for buffer in buffers {
            let slice = translate_guest_slice(bus, buffer.addr, buffer.len).ok_or(VirtioBlkStatus::IoErr)?;
            self.file.write_all(slice).map_err(|_| VirtioBlkStatus::IoErr)?;
        }
        self.file.flush().map_err(|_| VirtioBlkStatus::IoErr)?;
        Ok(0)
    }

    fn validate_range(&self, sector: u64, data_len: u32) -> Result<(), VirtioBlkStatus> {
        let sectors = u64::from(data_len) / SECTOR_SIZE;
        if u64::from(data_len) % SECTOR_SIZE != 0 {
            return Err(VirtioBlkStatus::IoErr);
        }
        if sector.checked_add(sectors).is_some_and(|end| end <= self.capacity) {
            Ok(())
        } else {
            Err(VirtioBlkStatus::IoErr)
        }
    }
}

impl MmioDevice for VirtioBlkDevice {
    fn read(&mut self, offset: usize, width: usize) -> Option<u64> {
        self.common.read(offset, width, &self.config_bytes())
    }

    fn write(&mut self, offset: usize, width: usize, value: u64) -> bool {
        self.common.write(offset, width, value, mem::size_of::<u64>())
    }

    fn update(&mut self, bus: &Bus) {
        self.handle_queue(bus);
    }

    fn interrupt_pending(&self) -> bool {
        self.common.interrupt_pending()
    }

    fn config_dtb(&self, builder: &mut DtbBuilder, config: &DtbConfig, addr: usize, len: usize, id: u32) {
        self.common.config_dtb(builder, config, addr, len, id);
    }
}

impl VirtioBlkStatus {
    fn value(self) -> u8 {
        self as u8
    }
}

fn validate_data_buffers(buffers: &[RequestBuffer], expected_write: bool) -> Result<u32, VirtioBlkStatus> {
    let mut data_len = 0u32;
    for buffer in buffers {
        if buffer.write != expected_write {
            return Err(VirtioBlkStatus::IoErr);
        }
        data_len = data_len.checked_add(buffer.len).ok_or(VirtioBlkStatus::IoErr)?;
    }
    if u64::from(data_len) % SECTOR_SIZE == 0 {
        Ok(data_len)
    } else {
        Err(VirtioBlkStatus::IoErr)
    }
}
