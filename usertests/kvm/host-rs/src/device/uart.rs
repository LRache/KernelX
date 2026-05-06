use std::collections::VecDeque;
use std::io::{self, Write};

use num_enum::TryFromPrimitive;

use crate::device::bus::{Bus, MmioDevice};
use crate::dtb::{DtbBuilder, DtbConfig, dtb_node_name, dtb_reg_cells};

pub struct Uart16650Device {
    queue: VecDeque<u8>,
    lsb: u8,
    msb: u8,
    ier: u8,
    iir: u8,
    lcr: u8,
    lsr: u8,
    msr: u8,
    interrupt: bool,
    thr_interrupt_pending: bool,
    stream_input_closed: bool,
    recv_fifo_trigger_byte_count: usize,
}

impl Default for Uart16650Device {
    fn default() -> Self {
        Self {
            queue: VecDeque::new(),
            lsb: 0,
            msb: 0,
            ier: 0,
            iir: InterruptIdentification::None.value(),
            lcr: 0b0000_0011,
            lsr: LineStatusBit::ThrEmpty.mask() | LineStatusBit::TransmitterEmpty.mask(),
            msr: 0,
            interrupt: false,
            thr_interrupt_pending: false,
            stream_input_closed: false,
            recv_fifo_trigger_byte_count: 1,
        }
    }
}

const UART_FIFO_CAPACITY: usize = 1024;

#[repr(u8)]
#[derive(Clone, Copy)]
enum LineControlBit {
    DivisorLatchAccess = 1 << 7,
}

impl LineControlBit {
    fn mask(self) -> u8 {
        self as u8
    }
}

#[repr(u8)]
#[derive(Clone, Copy)]
enum LineStatusBit {
    DataReady = 1 << 0,
    ThrEmpty = 1 << 5,
    TransmitterEmpty = 1 << 6,
}

impl LineStatusBit {
    fn mask(self) -> u8 {
        self as u8
    }
}

#[repr(u8)]
#[derive(Clone, Copy)]
enum InterruptEnableBit {
    RxAvailable = 1 << 0,
    ThrEmpty = 1 << 1,
}

impl InterruptEnableBit {
    fn mask(self) -> u8 {
        self as u8
    }
}

#[repr(u8)]
#[derive(Clone, Copy)]
enum InterruptIdentification {
    None = 0b1100_0001,
    ThrEmpty = 0b1100_0010,
    RxAvailable = 0b1100_0100,
}

impl InterruptIdentification {
    fn value(self) -> u8 {
        self as u8
    }
}

#[repr(usize)]
#[derive(Clone, Copy, TryFromPrimitive)]
enum UartRegister {
    RbrThrDll = 0,
    IerDlm = 1,
    IirFcr = 2,
    Lcr = 3,
    Mcr = 4,
    Lsr = 5,
    Msr = 6,
}

#[repr(u8)]
#[derive(Clone, Copy)]
enum FifoControlBit {
    ClearReceive = 1 << 1,
}

impl FifoControlBit {
    fn mask(self) -> u8 {
        self as u8
    }
}

impl Uart16650Device {
    pub const LENGTH: usize = 8;

    fn refresh_interrupt_state(&mut self) {
        if !self.queue.is_empty() && (self.ier & InterruptEnableBit::RxAvailable.mask()) != 0 {
            self.iir = InterruptIdentification::RxAvailable.value();
            self.interrupt = true;
            return;
        }
        if self.thr_interrupt_pending && (self.ier & InterruptEnableBit::ThrEmpty.mask()) != 0 {
            self.iir = InterruptIdentification::ThrEmpty.value();
            self.interrupt = true;
            return;
        }
        self.iir = InterruptIdentification::None.value();
        self.interrupt = false;
    }

    fn send_byte(&mut self, c: u8) {
        let mut stdout = io::stdout();
        let _ = stdout.write_all(&[c]);
        let _ = stdout.flush();
    }

    fn recv_byte(&mut self, c: u8) -> bool {
        if self.queue.len() >= UART_FIFO_CAPACITY {
            eprintln!("uart receive queue is full, dropping byte");
            return false;
        }
        self.queue.push_back(c);
        self.lsr |= LineStatusBit::DataReady.mask();
        self.refresh_interrupt_state();
        true
    }
}

impl MmioDevice for Uart16650Device {
    fn read(&mut self, offset: usize, width: usize) -> Option<u64> {
        if width != 1 {
            return None;
        }
        let value = match UartRegister::try_from(offset) {
            Ok(UartRegister::RbrThrDll) if (self.lcr & LineControlBit::DivisorLatchAccess.mask()) != 0 => {
                self.lsb as u64
            }
            Ok(UartRegister::RbrThrDll) => {
                let Some(c) = self.queue.pop_front() else {
                    return Some(u64::MAX);
                };
                if self.queue.is_empty() {
                    self.lsr &= !LineStatusBit::DataReady.mask();
                }
                self.refresh_interrupt_state();
                c as u64
            }
            Ok(UartRegister::IerDlm) if (self.lcr & LineControlBit::DivisorLatchAccess.mask()) != 0 => self.msb as u64,
            Ok(UartRegister::IerDlm) => self.ier as u64,
            Ok(UartRegister::IirFcr) => {
                let iir = self.iir;
                if (iir & 0x0f) == (InterruptIdentification::ThrEmpty.value() & 0x0f) {
                    self.thr_interrupt_pending = false;
                    self.refresh_interrupt_state();
                }
                iir as u64
            }
            Ok(UartRegister::Lcr) => self.lcr as u64,
            Ok(UartRegister::Lsr) => self.lsr as u64,
            Ok(UartRegister::Msr) => self.msr as u64,
            _ => 0,
        };
        Some(value)
    }

    fn write(&mut self, offset: usize, width: usize, value: u64) -> bool {
        if width != 1 {
            return false;
        }
        let byte = value as u8;
        match UartRegister::try_from(offset) {
            Ok(UartRegister::RbrThrDll) if (self.lcr & LineControlBit::DivisorLatchAccess.mask()) != 0 => {
                self.lsb = byte;
                true
            }
            Ok(UartRegister::RbrThrDll) => {
                self.send_byte(byte);
                if (self.lsr & LineStatusBit::ThrEmpty.mask()) != 0
                    && (self.ier & InterruptEnableBit::ThrEmpty.mask()) != 0
                {
                    self.thr_interrupt_pending = true;
                }
                self.refresh_interrupt_state();
                true
            }
            Ok(UartRegister::IerDlm) if (self.lcr & LineControlBit::DivisorLatchAccess.mask()) != 0 => {
                self.msb = byte;
                true
            }
            Ok(UartRegister::IerDlm) => {
                let old_ier = self.ier;
                self.ier = byte;
                if (old_ier & InterruptEnableBit::ThrEmpty.mask()) == 0
                    && (self.ier & InterruptEnableBit::ThrEmpty.mask()) != 0
                    && (self.lsr & LineStatusBit::ThrEmpty.mask()) != 0
                {
                    self.thr_interrupt_pending = true;
                }
                self.refresh_interrupt_state();
                true
            }
            Ok(UartRegister::IirFcr) => {
                if (byte & FifoControlBit::ClearReceive.mask()) != 0 {
                    self.queue.clear();
                    self.lsr &= !LineStatusBit::DataReady.mask();
                }
                self.recv_fifo_trigger_byte_count = match (byte >> 6) & 0x3 {
                    0 => 1,
                    1 => 4,
                    2 => 8,
                    _ => 14,
                };
                self.refresh_interrupt_state();
                true
            }
            Ok(UartRegister::Lcr) => {
                self.lcr = byte;
                true
            }
            Ok(UartRegister::Mcr) => true,
            _ => true,
        }
    }

    fn interrupt_pending(&self) -> bool {
        self.interrupt
    }

    fn clear_interrupt(&mut self) {
        self.refresh_interrupt_state();
    }

    fn config_dtb(&self, builder: &mut DtbBuilder, config: &DtbConfig, addr: usize, len: usize, id: u32) {
        builder.begin_node(&dtb_node_name("serial", addr));
        builder.prop_string("compatible", "ns16550a");
        builder.prop_string("status", "okay");
        builder.prop_cells("reg", &dtb_reg_cells(addr, len));
        builder.prop_u32("clock-frequency", 3_686_400);
        builder.prop_u32("current-speed", 115_200);
        builder.prop_u32("reg-shift", 0);
        builder.prop_u32("reg-io-width", 1);
        if id != 0 {
            builder.prop_u32("interrupt-parent", config.plic_phandle);
            builder.prop_u32("interrupts", id);
        }
        builder.end_node();
    }

    fn update(&mut self, _bus: &Bus) {
        if self.stream_input_closed {
            return;
        }

        let mut poll_fd = libc::pollfd {
            fd: libc::STDIN_FILENO,
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = unsafe { libc::poll(&mut poll_fd, 1, 0) };
        if ready <= 0 || (poll_fd.revents & libc::POLLIN) == 0 {
            return;
        }

        let mut buffer = [0u8; 64];
        let bytes = unsafe { libc::read(libc::STDIN_FILENO, buffer.as_mut_ptr().cast(), buffer.len()) };
        if bytes <= 0 {
            let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
            if bytes < 0 && (errno == libc::EINTR || errno == libc::EAGAIN || errno == libc::EWOULDBLOCK) {
                return;
            }
            self.stream_input_closed = true;
            return;
        }

        for &byte in &buffer[..bytes as usize] {
            self.recv_byte(byte);
        }
    }
}
