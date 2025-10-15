use alloc::collections::BinaryHeap;
use alloc::sync::Arc;
use core::cmp::Reverse;
use spin::Mutex;

use crate::kernel::event::Event;
use crate::kernel::task::TCB;
use crate::arch;
use crate::platform;

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

    pub fn add_timer(&self, tcb: Arc<TCB>, time: u64, event: Event) {
        let time = platform::get_time_us() + time;
        self.wait_queue.lock().push(Reverse(TimerEvent { time, tcb, event }));
    } 

    pub fn wakeup_expired(&self, current_time: u64) {
        let mut wait_queue = self.wait_queue.lock();
        while let Some(Reverse(event)) = wait_queue.peek() {
            if event.time <= current_time {
                let event = wait_queue.pop().unwrap().0;
                event.tcb.wakeup(Event::Timeout);
            } else {
                break;
            }
        }
    }
}

static TIMER: Timer = Timer::new();

pub fn init() {
    platform::set_next_timer_us(10000); // Set first timer interrupt in 10ms
    arch::enable_timer_interrupt();
}

pub fn add_timer(tcb: Arc<TCB>, time: u64) {
    TIMER.add_timer(tcb, time, Event::Timeout);
}

pub fn interrupt() {
    let current_time = platform::get_time_us();
    TIMER.wakeup_expired(current_time);

    platform::set_next_timer_us(10000); // Set next timer interrupt in 10ms
}
