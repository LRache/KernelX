use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::mem;

use num_enum::TryFromPrimitive;

use crate::device::bus::Bus;
use crate::device::virtio::{
    BackendOps, RequestBuffer, VirtioBackend, VirtioDeviceId, read_guest_u32, read_guest_u64, translate_guest_slice,
    translate_guest_slice_mut, write_guest_u8,
};

const SECTOR_SIZE: u64 = 512;
const QUEUE_COUNT: usize = 1;
const QUEUE_NUM_MAX: u32 = 128;

pub type VirtioBlkDevice = VirtioBackend<VirtioBlkBackend>;

pub struct VirtioBlkBackend {
    file: File,
    capacity: u64,
    config: [u8; mem::size_of::<u64>()],
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

impl VirtioBlkBackend {
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
            config: (image_size / SECTOR_SIZE).to_le_bytes(),
        })
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

        let result = VirtioBlkRequestType::try_from(request_type)
            .map_err(|_| VirtioBlkStatus::Unsupported)
            .and_then(|request_type| match request_type {
                VirtioBlkRequestType::In => self.blk_read(bus, sector, data_buffers),
                VirtioBlkRequestType::Out => self.blk_write(bus, sector, data_buffers),
                VirtioBlkRequestType::Flush => self.file.flush().map_err(|_| VirtioBlkStatus::IoErr).map(|_| 0),
                VirtioBlkRequestType::GetId
                | VirtioBlkRequestType::GetLifetime
                | VirtioBlkRequestType::Discard
                | VirtioBlkRequestType::WriteZeroes
                | VirtioBlkRequestType::SecureErase => Err(VirtioBlkStatus::Unsupported),
            });

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

impl VirtioBlkDevice {
    pub fn open(path: &str) -> Result<Self, String> {
        Ok(VirtioBackend::new(VirtioBlkBackend::open(path)?))
    }
}

impl BackendOps for VirtioBlkBackend {
    fn device_id(&self) -> VirtioDeviceId {
        VirtioDeviceId::Block
    }

    fn queue_count(&self) -> usize {
        QUEUE_COUNT
    }

    fn queue_num_max(&self) -> u32 {
        QUEUE_NUM_MAX
    }

    fn config(&self) -> &[u8] {
        &self.config
    }

    fn handle_request(&mut self, bus: &Bus, buffers: &[RequestBuffer]) -> Option<u32> {
        self.handle_blk_request(bus, buffers)
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
