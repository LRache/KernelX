use alloc::boxed::Box;
use alloc::collections::BinaryHeap;
use alloc::sync::Arc;
use core::cmp::Reverse;
use core::sync::atomic::{AtomicUsize, Ordering};
use core::time::Duration;

use crate::arch;
use crate::kernel::event::Event;
use crate::kernel::scheduler::Task;
use crate::kernel::{config, scheduler};
use crate::klib::SpinLock;

use super::event::TimerEvent;

struct Timer {
    wait_queue: SpinLock<BinaryHeap<Reverse<TimerEvent>>>,
    next_timer_id: AtomicUsize,
}

impl Timer {
    const fn new() -> Self {
        Self {
            wait_queue: SpinLock::new(BinaryHeap::new(), "Timer::wait_queue"),
            next_timer_id: AtomicUsize::new(0),
        }
    }

    pub fn add_timer(&self, time: Duration, callback: Box<dyn FnOnce()>) -> u64 {
        let time = arch::get_time_us() + time.as_micros() as u64;
        let new_id = self.next_timer_id.fetch_add(1, Ordering::Relaxed) as u64;
        self.wait_queue.lock().push(Reverse(TimerEvent {
            time,
            callback,
            id: new_id,
        }));
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
        self.wait_queue.lock().retain(|event| event.0.id != timer_id);
    }
}

unsafe impl Sync for Timer {}

static TIMER: Timer = Timer::new();

#[unsafe(link_section = ".text.init")]
pub fn init() {}

pub fn now() -> Duration {
    Duration::from_micros(arch::get_time_us())
}

pub fn add_timer(task: Arc<dyn Task>, time: Duration) -> u64 {
    TIMER.add_timer(
        time,
        Box::new(move || {
            let _ = scheduler::wakeup_task(task, Event::Timeout);
        }),
    )
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

    // TODO: Program timer interrupts from the earliest pending timer deadline.
    // The fixed 50ms tick can make nanosleep/usleep overshoot enough for timerfd
    // periodic reads to report one extra expiration in timerfd01.
    arch::set_next_time_event_us(config::TIMER_INTERRUPT_INTERVAL_US);
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
