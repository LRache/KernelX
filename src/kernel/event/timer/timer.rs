use alloc::collections::BinaryHeap;
use alloc::sync::Arc;
use core::cmp::Reverse;
use core::time::Duration;
use spin::Mutex;

use crate::kernel::event::Event;
use crate::kernel::scheduler::{self, Task};
use crate::arch;

use super::event::TimerEvent;

struct Timer {
    wait_queue: Mutex<BinaryHeap<Reverse<TimerEvent>>>,
}

impl Timer {
    const fn new() -> Self {
        Self {
            wait_queue: Mutex::new(BinaryHeap::new()),
        }
    }

    pub fn add_timer(&self, tcb: Arc<dyn Task>, time: Duration, event: Event) {
        let time = arch::get_time_us() + time.as_micros() as u64;
        self.wait_queue.lock().push(Reverse(TimerEvent { time, tcb, event }));
    } 

    pub fn wakeup_expired(&self, current_time: u64) {
        let mut wait_queue = self.wait_queue.lock();
        while let Some(Reverse(event)) = wait_queue.peek() {
            if event.time <= current_time {
                let event = wait_queue.pop().unwrap().0;
                scheduler::wakeup_task(event.tcb.clone(), Event::Timeout);
                // event.tcb.wakeup(Event::Timeout);
            } else {
                break;
            }
        }
    }
}

static TIMER: Timer = Timer::new();

#[unsafe(link_section = ".text.init")]
pub fn init() {
    arch::set_next_time_event_us(10000); // Set first timer interrupt in 10ms
    arch::enable_timer_interrupt();
}

pub fn now() -> Duration {
    Duration::from_micros(arch::get_time_us())
}

pub fn add_timer(tcb: Arc<dyn Task>, time: Duration) {
    TIMER.add_timer(tcb, time, Event::Timeout);
}

pub fn interrupt() {
    let current_time = arch::get_time_us();
    TIMER.wakeup_expired(current_time);

    arch::set_next_time_event_us(10000); // Set next timer interrupt in 10ms
}

pub fn wait_until(dur: Duration, mut f: impl FnMut() -> bool) -> bool {
    let start_time = arch::get_time_us();
    let us = dur.as_micros() as u64;
    loop {
        if f() {
            return true;
        }
        let current_time = arch::get_time_us();
        if current_time - start_time >= us {
            break;
        }
    }

    f()
}

pub fn spin_delay(dur: Duration) {
    let start_time = arch::get_time_us();
    let us = dur.as_micros() as u64;
    loop {
        let current_time = arch::get_time_us();
        if current_time - start_time >= us {
            break;
        }
    }
}
