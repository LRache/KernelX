use num_enum::TryFromPrimitive;
use std::io::SeekFrom;
use std::sync::{Arc, Mutex};
use std::{io, mem};
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::device::bus::{Bus, BusRef};
use crate::device::virtio::{
    BackendOps, RequestBuffer, VirtioBackend, VirtioCompletion, VirtioDeviceId, VirtioRequest, VirtioRequestHandling,
    read_guest_u32, read_guest_u64, translate_guest_slice, write_guest_slice, write_guest_u8,
};

const SECTOR_SIZE: u64 = 512;
const QUEUE_COUNT: usize = 1;
const QUEUE_NUM_MAX: u32 = 128;

pub type VirtioBlkDevice = VirtioBackend<VirtioBlkBackend>;

pub struct VirtioBlkBackend {
    capacity: u64,
    config: [u8; mem::size_of::<u64>()],
    request_tx: mpsc::UnboundedSender<BlkIoRequest>,
    completion_tx: mpsc::UnboundedSender<BlkIoCompletion>,
    completion_rx: Arc<Mutex<mpsc::UnboundedReceiver<BlkIoCompletion>>>,
    worker_state: Arc<Mutex<Option<BlkIoWorkerState>>>,
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
#[derive(Clone, Copy, PartialEq, Eq)]
enum VirtioBlkStatus {
    Ok = 0,
    IoErr = 1,
    Unsupported = 2,
}

struct BlkIoWorkerState {
    file: File,
    request_rx: mpsc::UnboundedReceiver<BlkIoRequest>,
}

struct BlkIoRequest {
    queue_idx: usize,
    desc_idx: u16,
    status_addr: u64,
    operation: BlkIoOperation,
}

enum BlkIoOperation {
    Read {
        sector: u64,
        data_len: u32,
        buffers: Vec<RequestBuffer>,
    },
    Write {
        sector: u64,
        data: Vec<u8>,
    },
    Flush,
}

struct BlkIoCompletion {
    queue_idx: usize,
    desc_idx: u16,
    status_addr: u64,
    status: VirtioBlkStatus,
    data_len: u32,
    read_data: Option<Vec<u8>>,
    read_buffers: Vec<RequestBuffer>,
}

enum PreparedBlkRequest {
    Async(BlkIoRequest),
    Complete {
        status_addr: u64,
        status: VirtioBlkStatus,
        data_len: u32,
    },
}

impl VirtioBlkBackend {
    pub async fn open(path: &str) -> Result<Self, String> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .await
            .map_err(|err| format!("open virtio-blk image {path}: {err}"))?;
        let image_size = file
            .metadata()
            .await
            .map_err(|err| format!("stat virtio-blk image {path}: {err}"))?
            .len();
        if image_size < SECTOR_SIZE {
            return Err(format!("virtio-blk image {path} is smaller than one sector"));
        }
        let (request_tx, request_rx) = mpsc::unbounded_channel();
        let (completion_tx, completion_rx) = mpsc::unbounded_channel();
        Ok(Self {
            capacity: image_size / SECTOR_SIZE,
            config: (image_size / SECTOR_SIZE).to_le_bytes(),
            request_tx,
            completion_tx,
            completion_rx: Arc::new(Mutex::new(completion_rx)),
            worker_state: Arc::new(Mutex::new(Some(BlkIoWorkerState { file, request_rx }))),
        })
    }

    fn handle_blk_request(&mut self, bus: &Bus, request: &VirtioRequest) -> VirtioRequestHandling {
        let Some(prepared) = self.prepare_blk_request(bus, request) else {
            return VirtioRequestHandling::Complete(0);
        };
        match prepared {
            PreparedBlkRequest::Async(request) => match self.request_tx.send(request) {
                Ok(()) => VirtioRequestHandling::Pending,
                Err(err) => Self::complete_status(bus, err.0.status_addr, VirtioBlkStatus::IoErr, 0),
            },
            PreparedBlkRequest::Complete {
                status_addr,
                status,
                data_len,
            } => Self::complete_status(bus, status_addr, status, data_len),
        }
    }

    fn prepare_blk_request(&self, bus: &Bus, request: &VirtioRequest) -> Option<PreparedBlkRequest> {
        let buffers = &request.buffers;
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

        match VirtioBlkRequestType::try_from(request_type) {
            Ok(VirtioBlkRequestType::In) => self.prepare_blk_read(bus, request, status.addr, sector, data_buffers),
            Ok(VirtioBlkRequestType::Out) => self.prepare_blk_write(bus, request, status.addr, sector, data_buffers),
            Ok(VirtioBlkRequestType::Flush) => Some(PreparedBlkRequest::Async(BlkIoRequest {
                queue_idx: request.queue_idx,
                desc_idx: request.desc_idx,
                status_addr: status.addr,
                operation: BlkIoOperation::Flush,
            })),
            Ok(
                VirtioBlkRequestType::GetId
                | VirtioBlkRequestType::GetLifetime
                | VirtioBlkRequestType::Discard
                | VirtioBlkRequestType::WriteZeroes
                | VirtioBlkRequestType::SecureErase,
            )
            | Err(_) => Some(PreparedBlkRequest::Complete {
                status_addr: status.addr,
                status: VirtioBlkStatus::Unsupported,
                data_len: 0,
            }),
        }
    }

    fn prepare_blk_read(
        &self,
        bus: &Bus,
        request: &VirtioRequest,
        status_addr: u64,
        sector: u64,
        buffers: &[RequestBuffer],
    ) -> Option<PreparedBlkRequest> {
        let data_len = match validate_data_buffers(buffers, true).and_then(|len| {
            self.validate_range(sector, len)?;
            validate_guest_buffers(bus, buffers)?;
            Ok(len)
        }) {
            Ok(data_len) => data_len,
            Err(status) => {
                return Some(PreparedBlkRequest::Complete {
                    status_addr,
                    status,
                    data_len: 0,
                });
            }
        };
        Some(PreparedBlkRequest::Async(BlkIoRequest {
            queue_idx: request.queue_idx,
            desc_idx: request.desc_idx,
            status_addr,
            operation: BlkIoOperation::Read {
                sector,
                data_len,
                buffers: buffers.to_vec(),
            },
        }))
    }

    fn prepare_blk_write(
        &self,
        bus: &Bus,
        request: &VirtioRequest,
        status_addr: u64,
        sector: u64,
        buffers: &[RequestBuffer],
    ) -> Option<PreparedBlkRequest> {
        let data = match self.collect_write_data(bus, sector, buffers) {
            Ok(data) => data,
            Err(status) => {
                return Some(PreparedBlkRequest::Complete {
                    status_addr,
                    status,
                    data_len: 0,
                });
            }
        };
        Some(PreparedBlkRequest::Async(BlkIoRequest {
            queue_idx: request.queue_idx,
            desc_idx: request.desc_idx,
            status_addr,
            operation: BlkIoOperation::Write { sector, data },
        }))
    }

    fn collect_write_data(
        &self,
        bus: &Bus,
        sector: u64,
        buffers: &[RequestBuffer],
    ) -> Result<Vec<u8>, VirtioBlkStatus> {
        let data_len = validate_data_buffers(buffers, false)?;
        self.validate_range(sector, data_len)?;
        let mut data = Vec::with_capacity(usize::try_from(data_len).map_err(|_| VirtioBlkStatus::IoErr)?);
        for buffer in buffers {
            let slice = translate_guest_slice(bus, buffer.addr, buffer.len).ok_or(VirtioBlkStatus::IoErr)?;
            data.extend_from_slice(slice);
        }
        Ok(data)
    }

    fn pop_blk_completion(&mut self, bus: &Bus) -> Option<VirtioCompletion> {
        let completion = self
            .completion_rx
            .lock()
            .expect("virtio-blk completion queue lock poisoned")
            .try_recv()
            .ok()?;
        Some(self.finish_blk_completion(bus, completion))
    }

    fn finish_blk_completion(&mut self, bus: &Bus, completion: BlkIoCompletion) -> VirtioCompletion {
        let mut status = completion.status;
        if status == VirtioBlkStatus::Ok
            && let Some(data) = completion.read_data.as_deref()
            && write_read_data(bus, &completion.read_buffers, data).is_err()
        {
            status = VirtioBlkStatus::IoErr;
        }

        let data_len = if status == VirtioBlkStatus::Ok {
            completion.data_len
        } else {
            0
        };
        let len = match write_guest_u8(bus, completion.status_addr, status.value()) {
            Some(()) => data_len.checked_add(1).unwrap_or(1),
            None => 0,
        };

        VirtioCompletion {
            queue_idx: completion.queue_idx,
            desc_idx: completion.desc_idx,
            len,
        }
    }

    fn complete_status(bus: &Bus, status_addr: u64, status: VirtioBlkStatus, data_len: u32) -> VirtioRequestHandling {
        let len = match write_guest_u8(bus, status_addr, status.value()) {
            Some(()) => {
                if status == VirtioBlkStatus::Ok {
                    data_len.checked_add(1).unwrap_or(1)
                } else {
                    1
                }
            }
            None => 0,
        };
        VirtioRequestHandling::Complete(len)
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
    pub async fn open(path: &str) -> Result<Self, String> {
        Ok(VirtioBackend::new(VirtioBlkBackend::open(path).await?))
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

    fn handle_request(&mut self, bus: &Bus, request: &VirtioRequest) -> VirtioRequestHandling {
        self.handle_blk_request(bus, request)
    }

    fn pop_completion(&mut self, bus: &Bus) -> Option<VirtioCompletion> {
        self.pop_blk_completion(bus)
    }

    fn spawn_tasks(&self, bus: BusRef) -> Vec<JoinHandle<()>> {
        let Some(worker_state) = self
            .worker_state
            .lock()
            .expect("virtio-blk worker lock poisoned")
            .take()
        else {
            return Vec::new();
        };
        vec![tokio::spawn(blk_io_task(worker_state, self.completion_tx.clone(), bus))]
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

fn validate_guest_buffers(bus: &Bus, buffers: &[RequestBuffer]) -> Result<(), VirtioBlkStatus> {
    for buffer in buffers {
        if translate_guest_slice(bus, buffer.addr, buffer.len).is_none() {
            return Err(VirtioBlkStatus::IoErr);
        }
    }
    Ok(())
}

fn write_read_data(bus: &Bus, buffers: &[RequestBuffer], data: &[u8]) -> Result<(), VirtioBlkStatus> {
    let mut offset = 0usize;
    for buffer in buffers {
        let len = usize::try_from(buffer.len).map_err(|_| VirtioBlkStatus::IoErr)?;
        let end = offset.checked_add(len).ok_or(VirtioBlkStatus::IoErr)?;
        write_guest_slice(bus, buffer.addr, data.get(offset..end).ok_or(VirtioBlkStatus::IoErr)?)
            .ok_or(VirtioBlkStatus::IoErr)?;
        offset = end;
    }
    if offset == data.len() {
        Ok(())
    } else {
        Err(VirtioBlkStatus::IoErr)
    }
}

async fn blk_io_task(
    mut worker_state: BlkIoWorkerState,
    completion_tx: mpsc::UnboundedSender<BlkIoCompletion>,
    bus: BusRef,
) {
    while let Some(request) = worker_state.request_rx.recv().await {
        let completion = handle_io_request(&mut worker_state.file, request).await;
        if completion_tx.send(completion).is_err() {
            return;
        }
        if let Err(err) = Bus::notify(&bus).await {
            eprintln!("{err}");
            return;
        }
    }
}

async fn handle_io_request(file: &mut File, request: BlkIoRequest) -> BlkIoCompletion {
    let queue_idx = request.queue_idx;
    let desc_idx = request.desc_idx;
    let status_addr = request.status_addr;
    let mut completion = BlkIoCompletion {
        queue_idx,
        desc_idx,
        status_addr,
        status: VirtioBlkStatus::Ok,
        data_len: 0,
        read_data: None,
        read_buffers: Vec::new(),
    };

    completion.status = match request.operation {
        BlkIoOperation::Read {
            sector,
            data_len,
            buffers,
        } => {
            completion.data_len = data_len;
            completion.read_buffers = buffers;
            match read_blocks(file, sector, data_len).await {
                Ok(data) => {
                    completion.read_data = Some(data);
                    VirtioBlkStatus::Ok
                }
                Err(_) => {
                    completion.data_len = 0;
                    VirtioBlkStatus::IoErr
                }
            }
        }
        BlkIoOperation::Write { sector, data } => write_blocks(file, sector, &data)
            .await
            .map(|_| VirtioBlkStatus::Ok)
            .unwrap_or(VirtioBlkStatus::IoErr),
        BlkIoOperation::Flush => file
            .flush()
            .await
            .map(|_| VirtioBlkStatus::Ok)
            .unwrap_or(VirtioBlkStatus::IoErr),
    };

    completion
}

async fn read_blocks(file: &mut File, sector: u64, data_len: u32) -> io::Result<Vec<u8>> {
    seek_sector(file, sector).await?;
    let mut data = vec![0; usize::try_from(data_len).map_err(|_| io::ErrorKind::InvalidInput)?];
    file.read_exact(&mut data).await?;
    Ok(data)
}

async fn write_blocks(file: &mut File, sector: u64, data: &[u8]) -> io::Result<()> {
    seek_sector(file, sector).await?;
    file.write_all(data).await?;
    file.flush().await
}

async fn seek_sector(file: &mut File, sector: u64) -> io::Result<()> {
    let offset = sector.checked_mul(SECTOR_SIZE).ok_or(io::ErrorKind::InvalidInput)?;
    file.seek(SeekFrom::Start(offset)).await.map(|_| ())
}
