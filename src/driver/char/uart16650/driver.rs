// Reference: https://github.com/mit-pdos/xv6-riscv/blob/riscv/kernel/uart.c

use alloc::string::String;
use alloc::sync::Arc;

use crate::driver::{DriverOps, CharDriverOps, DeviceType};
use crate::kernel::errno::SysResult;
use crate::kernel::event::{Event, FileEvent, PollEventSet, WaitQueue};
use crate::kernel::scheduler::current;
use crate::klib::SpinLock;
use crate::klib::ring::RingBuffer;

mod regs {
    pub const RHR: usize = 0; // receive holding register (for input bytes)
    pub const THR: usize = 0; // transmit holding register (for output bytes)
    pub const IER: usize = 1; // interrupt enable register
    pub const IER_RX_ENABLE: u8 = 1 << 0;
    pub const IER_TX_ENABLE: u8 = 1 << 1;
    pub const FCR: usize = 2; // FIFO control register
    pub const FCR_FIFO_ENABLE: u8 = 1 << 0;
    pub const FCR_FIFO_CLEAR: u8 = 3 << 1; // clear the content of the two FIFOs
    pub const ISR: usize = 2; // interrupt status register
    pub const LCR: usize = 3; // line control register
    pub const LCR_EIGHT_BITS: u8 = 3 << 0;
    pub const LCR_BAUD_LATCH: u8 = 1 << 7; // special mode to set baud rate
    pub const LSR: usize = 5; // line status register
    pub const LSR_RX_READY: u8 = 1 << 0; // input is waiting to be read from RHR
    pub const LSR_TX_IDLE: u8 = 1 << 5; // THR can accept another character to send
}

struct MMIO {
    base: usize,
}

impl MMIO {
    #[inline(always)]
    fn read_reg(&mut self, offset: usize) -> u8 {
        unsafe { core::ptr::read_volatile((self.base + offset) as *const u8) }
    }

    #[inline(always)]
    fn write_reg(&mut self, offset: usize, value: u8) {
        unsafe { core::ptr::write_volatile((self.base + offset) as *mut u8, value); }
    }
}

pub struct Driver {
    name: String,
    mmio: SpinLock<MMIO>,
    recv_buffer: SpinLock<RingBuffer<u8, 1024>>,
    waiters: SpinLock<WaitQueue<usize>>,
}

impl Driver {
    pub fn new(base: usize, name: String) -> Self {
        Driver {
            name,
            mmio: SpinLock::new(MMIO { base }),
            recv_buffer: SpinLock::new(RingBuffer::new(0)),
            waiters: SpinLock::new(WaitQueue::new()),
        }
    }

    pub fn init(&self) {
        let mut mmio = self.mmio.lock();

        // Disable interrupts while configuring the UART.
        mmio.write_reg(regs::IER, 0x00);

        // Enter baud latch mode to configure baud rate.
        mmio.write_reg(regs::LCR, regs::LCR_BAUD_LATCH);

        // Configure baud rate divisor for 38.4K (LSB then MSB).
        mmio.write_reg(0, 0x03);
        mmio.write_reg(1, 0x00);
        // Leave baud latch mode and configure: 8 bits, no parity.
        mmio.write_reg(regs::LCR, regs::LCR_EIGHT_BITS);

        // Reset and enable FIFOs.
        mmio.write_reg(regs::FCR, regs::FCR_FIFO_ENABLE | regs::FCR_FIFO_CLEAR);

        // Enable receive interrupts.
        mmio.write_reg(regs::IER, regs::IER_RX_ENABLE);
    }
}

impl DriverOps for Driver {
    fn name(&self) -> &str {
        "uart16650"
    }

    fn device_name(&self) -> String {
        self.name.clone()
    }

    fn device_type(&self) -> DeviceType {
        DeviceType::Char
    }

    fn as_char_driver(self: Arc<Self>) -> Option<Arc<dyn CharDriverOps>> {
        Some(self)
    }

    fn handle_interrupt(&self) {
        let mut mmio = self.mmio.lock();
        // Check if there is data to read.
        if (mmio.read_reg(regs::LSR) & regs::LSR_RX_READY) != 0 {
            let mut recv_buffer = self.recv_buffer.lock();
            while mmio.read_reg(regs::LSR) & regs::LSR_RX_READY != 0 {
                let c = mmio.read_reg(regs::RHR);
                recv_buffer.push(c);
            }
            drop(recv_buffer);

            self.waiters.lock().wake_all(
                |waker| Event::Poll { event: FileEvent::ReadReady, waker }
            );
        }
    }
}

impl CharDriverOps for Driver {
    fn write(&self, buf: &[u8]) -> SysResult<usize> {
        let mut mmio = self.mmio.lock();
        for &c in buf {
            // Wait for UART to be ready to transmit.
            while (mmio.read_reg(regs::LSR) & regs::LSR_TX_IDLE) == 0 {}
            
            mmio.write_reg(regs::THR, c);
        }

        Ok(buf.len())
    }

    fn read(&self, buf: &mut [u8]) -> SysResult<usize> {
        let mut idx = 0;

        let mut recv_buffer = self.recv_buffer.lock();
        for i in 0..buf.len() {
            if let Some(c) = recv_buffer.pop() {
                buf[i] = c;
                idx += 1;
            } else {
                break;
            }
        }
        drop(recv_buffer);

        let mut mmio = self.mmio.lock();
        for i in idx..buf.len() {
            if (mmio.read_reg(regs::LSR) & regs::LSR_RX_READY) == 0 {
                break;
            } else {
                buf[i] = mmio.read_reg(regs::RHR);
            }
        }
        
        Ok(idx)
    }

    fn wait_event(&self, waker: usize, event: PollEventSet) -> SysResult<Option<FileEvent>> {
        if event.contains(PollEventSet::POLLOUT) {
            return Ok(Some(FileEvent::WriteReady));
        }
        
        if event.contains(PollEventSet::POLLIN) {
            if self.recv_buffer.lock().empty() {
                self.waiters.lock().wait_current(waker);
                return Ok(None);
            } else {
                return Ok(Some(FileEvent::ReadReady));
            }
        }
        
        Ok(None)
    }

    fn wait_event_cancel(&self) {
        self.waiters.lock().remove(current::task());
    }
}
