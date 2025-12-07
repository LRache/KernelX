use alloc::boxed::Box;
use alloc::collections::BinaryHeap;
use alloc::sync::Arc;
use core::cmp::Reverse;
use core::time::Duration;
use spin::Mutex;

use crate::kernel::event::Event;
use crate::kernel::scheduler;
use crate::kernel::scheduler::Task;
use crate::arch;
use crate::klib::SpinLock;

use super::event::TimerEvent;

struct Timer {
    wait_queue: Mutex<BinaryHeap<Reverse<TimerEvent>>>,
    next_timer_id: SpinLock<u64>,
}

impl Timer {
    const fn new() -> Self {
        Self {
            wait_queue: Mutex::new(BinaryHeap::new()),
            next_timer_id: SpinLock::new(0),
        }
    }

    pub fn add_timer(&self, time: Duration, callback: Box<dyn FnOnce()>) -> u64 {
        let time = arch::get_time_us() + time.as_micros() as u64;
        let new_id = {
            let mut id_lock = self.next_timer_id.lock();
            *id_lock += 1;
            *id_lock
        };
        self.wait_queue.lock().push(Reverse(TimerEvent { time, callback, id: new_id }));
        new_id
    } 

    pub fn wakeup_expired(&self, current_time: u64) {
        loop {
            let mut wait_queue = self.wait_queue.lock();
            if let Some(Reverse(event)) = wait_queue.peek() {
                if event.time <= current_time {
                    let event = wait_queue.pop().unwrap().0;
                    drop(wait_queue);
                    (event.callback)();
                } else {
                    break;
                }
            } else {
                break;
            }
        }
    }

    pub fn remove(&self, timer_id: u64) {
        self.wait_queue.lock()
                       .retain(|event| event.0.id != timer_id);
    }
}

unsafe impl Sync for Timer {}

static TIMER: Timer = Timer::new();

#[unsafe(link_section = ".text.init")]
pub fn init() {
    arch::set_next_time_event_us(10000); // Set first timer interrupt in 10ms
    arch::enable_timer_interrupt();
}

pub fn now() -> Duration {
    Duration::from_micros(arch::get_time_us())
}

pub fn add_timer(task: Arc<dyn Task>, time: Duration) -> u64 {
    TIMER.add_timer(time, Box::new(move || {
        scheduler::wakeup_task(task, Event::Timeout);
    }))
}

pub fn add_timer_with_callback(time: Duration, callback: Box<dyn FnOnce()>) -> u64 {
    TIMER.add_timer(time, callback)
}

pub fn remove_timer(timer_id: u64) {
    TIMER.remove(timer_id);
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
