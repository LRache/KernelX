use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::{Event, WaitQueue};
use crate::kernel::scheduler::current;
use crate::klib::SpinLock;

pub(super) struct FanotifyPermission {
    response: SpinLock<Option<SysResult<()>>>,
    waiter: SpinLock<WaitQueue<Event>>,
}

impl FanotifyPermission {
    pub(super) fn new() -> Self {
        Self {
            response: SpinLock::new(None, "FanotifyPermission::response"),
            waiter: SpinLock::new(WaitQueue::new(), "FanotifyPermission::waiter"),
        }
    }

    pub(super) fn wait(&self) -> SysResult<()> {
        loop {
            let response = self.response.lock();
            if let Some(response) = *response {
                return response;
            }

            self.waiter.lock().wait_current(Event::FanotifyPermission);
            drop(response);

            current::schedule();
            match current::task().take_wakeup_event().unwrap() {
                Event::FanotifyPermission => {}
                Event::Signal => {
                    self.waiter.lock().remove(current::task());
                    return Err(Errno::EINTR);
                }
                event => unreachable!("unexpected event while waiting on fanotify permission: {:?}", event),
            }
        }
    }

    pub(super) fn respond(&self, response: SysResult<()>) {
        *self.response.lock() = Some(response);
        self.waiter.lock().wake_all(|event| event);
    }
}
