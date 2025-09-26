use alloc::collections::VecDeque;
use spin::Mutex;

use super::WaitQueue;

pub struct Pipe {
    buffer: Mutex<VecDeque<u8>>,
    read_waiter: WaitQueue,
}
