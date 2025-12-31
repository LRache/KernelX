pub struct RingBuffer<T: Copy, const N: usize> {
    buffer: [T; N],
    head: usize,
    tail: usize,
}

impl<T: Copy, const N: usize> RingBuffer<T, N> {
    pub fn new(default: T) -> Self {
        RingBuffer {
            buffer: [default; N],
            head: 0,
            tail: 0,
        }
    }

    pub fn empty(&self) -> bool {
        self.head == self.tail
    }

    pub fn push(&mut self, item: T) {
        let next_head = (self.head + 1) % N;
        if next_head == self.tail {
            self.tail = (self.tail + 1) % N; // Overwrite the oldest item
        }
        self.buffer[self.head] = item;
        self.head = next_head;
    }

    pub fn pop(&mut self) -> Option<T> {
        if self.head == self.tail {
            return None; // Buffer is empty
        }
        let item = self.buffer[self.tail];
        self.tail = (self.tail + 1) % N;
        Some(item)
    }

    pub fn clear(&mut self) {
        self.head = 0;
        self.tail = 0;
    }
}
