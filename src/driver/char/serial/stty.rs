use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use core::usize;

use crate::driver::char::serial::SerialOps;
use crate::driver::char::tty::{TtyInputEvent, TtyIoctlResult, TtyState};
use crate::driver::{CharDriverOps, DeviceType, DriverOps};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::{EpollNotifier, Event, FileEvent, WaitQueue};
use crate::kernel::ipc::{KSiFields, SiCode, signum};
use crate::kernel::mm::AddrSpace;
use crate::kernel::scheduler::current;
use crate::klib::SpinLock;

pub struct Stty {
    name: String,
    serial: SpinLock<Box<dyn SerialOps>>,
    waiters: SpinLock<WaitQueue<Event>>,
    epoll_notifier: Arc<EpollNotifier>,
    tty: TtyState,
}

impl Stty {
    pub fn new(name: String, serial: Box<dyn SerialOps>) -> Self {
        Stty {
            name,
            serial: SpinLock::new(serial, "Stty::serial"),
            waiters: SpinLock::new(WaitQueue::new("Stty::waiters"), "Stty::waiters"),
            epoll_notifier: Arc::new(EpollNotifier::new()),
            tty: TtyState::new(),
        }
    }
}

impl DriverOps for Stty {
    fn name(&self) -> &str {
        "stty"
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
        let mut serial = self.serial.lock();
        serial.acknowledge_interrupt();

        while let Some(c) = serial.getchar() {
            let event = self.tty.process_input_byte(c, |c| {
                let _ = serial.putchar(c);
            });
            if matches!(event, Some(TtyInputEvent::Interrupt)) && current::has_task() {
                let _ = current::pcb().send_signal(signum::SIGQUIT, SiCode::EMPTY, 0, KSiFields::Empty, None);
            }
        }
        let read_ready = self.tty.input_ready();

        self.waiters.lock().wake_all(|e| e);
        if read_ready {
            self.epoll_notifier.notify(FileEvent::READ_READY);
        }
    }
}

impl CharDriverOps for Stty {
    fn write(&self, buf: &[u8]) -> SysResult<usize> {
        let mut serial = self.serial.lock();
        for &c in buf {
            self.tty.process_output_byte(c, |c| while !serial.putchar(c) {});
        }
        Ok(buf.len())
    }

    fn read(&self, buf: &mut [u8], blocked: bool) -> SysResult<usize> {
        if blocked {
            loop {
                // Lock `waiters` first to avoid losing wakeup window.
                let mut waiters = self.waiters.lock();

                if let Some(read) = self.tty.read_input(buf) {
                    return Ok(read);
                }

                waiters.wait_current(Event::ReadReady);
                drop(waiters);

                current::schedule();
                match current::task().take_wakeup_event().unwrap() {
                    Event::ReadReady => {}
                    Event::Signal => {
                        self.waiters.lock().remove_current();
                        return Err(Errno::EINTR);
                    }
                    _ => unreachable!(),
                }
            }
        }

        self.tty.read_input(buf).ok_or(Errno::EAGAIN)
    }

    fn poll_event(&self, event: FileEvent) -> SysResult<Option<FileEvent>> {
        let mut ready = FileEvent::empty();

        if event.contains(FileEvent::WRITE_READY) {
            ready |= FileEvent::WRITE_READY;
        }

        if event.contains(FileEvent::READ_READY) && self.tty.input_ready() {
            ready |= FileEvent::READ_READY;
        }

        if ready.is_empty() { Ok(None) } else { Ok(Some(ready)) }
    }

    fn wait_event(&self, waker: usize, event: FileEvent) -> SysResult<Option<FileEvent>> {
        // Lock `waiters` first to avoid losing wakeup window.
        let mut waiters = self.waiters.lock();

        if let Some(ready) = self.poll_event(event)? {
            return Ok(Some(ready));
        }

        if event.contains(FileEvent::READ_READY) {
            waiters.wait_pending(
                current::task().clone(),
                Event::Poll {
                    event: FileEvent::READ_READY,
                    waker,
                },
            );
        }

        Ok(None)
    }

    fn wait_event_cancel(&self) {
        self.waiters.lock().remove(current::task());
    }

    fn epoll_notifier(&self) -> Option<Arc<EpollNotifier>> {
        Some(self.epoll_notifier.clone())
    }

    fn ioctl(&self, request: usize, arg: usize, addrspace: &AddrSpace) -> SysResult<usize> {
        match self.tty.ioctl(request, arg, addrspace)? {
            TtyIoctlResult::Handled(ret) => Ok(ret),
            TtyIoctlResult::FlushInput(ret) => {
                self.tty.clear_input();
                Ok(ret)
            }
            TtyIoctlResult::Unsupported => Err(Errno::ENOTTY),
        }
    }
}
