//! Hashed-bucket futex implementation (Linux model).
//!
//! All futex state is sharded over `BUCKET_COUNT` buckets, each protected by
//! its own `SpinLock`. Because bucket locks are spinlocks, no code path may
//! fault (and thus sleep) while holding one. The user futex word is therefore
//! translated and pinned *outside* the bucket lock (`FutexWord::pin`, which
//! may fault the page in), and re-read under the lock through the pinned
//! kernel mapping, which cannot fault.
//!
//! Lock ordering:
//! - multiple bucket locks are only ever acquired in ascending bucket-index
//!   order (requeue, waitv);
//! - a waiter's `location` spinlock is only acquired while holding the bucket
//!   lock (or with no bucket lock held), never the other way around.

use alloc::collections::{BTreeMap, LinkedList};
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::Event;
use crate::kernel::mm::AddrSpace;
use crate::kernel::mm::maparea::ReadChunk;
use crate::kernel::scheduler;
use crate::kernel::scheduler::{Task, current};
use crate::klib::SpinLock;

const BUCKET_BITS: usize = 6;
const BUCKET_COUNT: usize = 1 << BUCKET_BITS; // 64

struct FutexWaitQueueItem {
    tcb: Arc<dyn Task>,
    bitset: u32,
    kind: FutexWaitKind,
    /// The key this waiter is currently queued under. FUTEX_REQUEUE may move
    /// a sleeping waiter to another key (possibly in another bucket); it
    /// rewrites this cell while holding both bucket locks so that
    /// `cancel_wait` can always find the entry on timeout/EINTR.
    location: Arc<SpinLock<FutexKey>>,
}

#[derive(Clone, Copy)]
enum FutexWaitKind {
    Wait,
    Waitv { index: usize },
}

fn wake_waiter(item: FutexWaitQueueItem) -> bool {
    let event = match item.kind {
        FutexWaitKind::Wait => Event::Futex,
        FutexWaitKind::Waitv { index } => Event::FutexWaitv { index },
    };

    scheduler::wakeup_task(item.tcb, event).is_ok()
}

struct Futex {
    wait_list: LinkedList<FutexWaitQueueItem>,
}

impl Futex {
    fn new() -> Self {
        Self {
            wait_list: LinkedList::new(),
        }
    }

    fn wake(&mut self, num: usize, mask: u32) -> usize {
        if num == 0 {
            return 0;
        }

        let mut woken = 0;
        let mut cursor = self.wait_list.cursor_front_mut();
        while let Some(item) = cursor.current() {
            if (item.bitset & mask) != 0 {
                let item = cursor.remove_current().unwrap();

                if wake_waiter(item) {
                    woken += 1;
                    if woken >= num {
                        break;
                    }
                }
            } else {
                cursor.move_next();
            }
        }

        woken
    }

    fn remove_waiter(&mut self, task: &Arc<dyn Task>) -> usize {
        let mut removed = 0;
        let mut cursor = self.wait_list.cursor_front_mut();
        while let Some(item) = cursor.current() {
            if Arc::ptr_eq(&item.tcb, task) {
                cursor.remove_current();
                removed += 1;
            } else {
                cursor.move_next();
            }
        }
        removed
    }

    fn is_empty(&self) -> bool {
        self.wait_list.is_empty()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FutexKey {
    Private { addrspace: usize, uaddr: usize },
    Shared { page: usize, offset: usize },
}

impl FutexKey {
    pub fn private(addrspace: &Arc<AddrSpace>, uaddr: usize) -> Self {
        Self::Private {
            addrspace: Arc::as_ptr(addrspace) as usize,
            uaddr,
        }
    }

    pub fn shared(frame: &crate::kernel::mm::maparea::PinPageFrame, offset: usize) -> Self {
        Self::Shared {
            page: frame.kpage(),
            offset,
        }
    }
}

struct Bucket {
    futexes: BTreeMap<FutexKey, Futex>,
}

impl Bucket {
    const fn new() -> Self {
        Self {
            futexes: BTreeMap::new(),
        }
    }

    fn enqueue(&mut self, key: FutexKey, item: FutexWaitQueueItem) {
        self.futexes.entry(key).or_insert_with(Futex::new).wait_list.push_back(item);
    }

    /// Drops the map entry for `key` if its wait list became empty.
    fn cleanup(&mut self, key: &FutexKey) {
        if self.futexes.get(key).is_some_and(Futex::is_empty) {
            self.futexes.remove(key);
        }
    }
}

static BUCKETS: [SpinLock<Bucket>; BUCKET_COUNT] =
    [const { SpinLock::new(Bucket::new(), "futex_bucket") }; BUCKET_COUNT];

fn bucket_of(key: &FutexKey) -> usize {
    let (base, addr) = match *key {
        FutexKey::Private { addrspace, uaddr } => (addrspace, uaddr),
        FutexKey::Shared { page, offset } => (page, offset),
    };
    // Fibonacci multiplicative hash over the mixed key halves; the top bits
    // give the best-distributed BUCKET_BITS.
    let hash = (base ^ (addr >> 2)).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    hash >> (usize::BITS as usize - BUCKET_BITS)
}

/// The user futex word, translated and pinned to its physical page.
///
/// `pin` may fault the page in (allocation / CoW / backing I/O) and therefore
/// sleep — it must be called outside any bucket lock. `read` goes through the
/// pinned kernel mapping and can never fault, so it is safe under a bucket
/// spinlock. Callers keep the `FutexWord` alive on their stack across the
/// whole wait, so the page stays resident while the waiter is queued.
pub struct FutexWord {
    chunk: ReadChunk,
}

impl FutexWord {
    pub fn pin(addrspace: &AddrSpace, uaddr: usize) -> SysResult<Self> {
        debug_assert!(uaddr & 3 == 0);
        Ok(Self {
            chunk: addrspace.translate_read(uaddr, core::mem::size_of::<i32>())?,
        })
    }

    pub fn read(&self) -> i32 {
        // Volatile: user space mutates the word concurrently from other harts.
        unsafe { core::ptr::read_volatile(self.chunk.as_ptr().cast::<i32>()) }
    }
}

/// Handle to a queued waiter, used to dequeue it on timeout/EINTR even if a
/// concurrent FUTEX_REQUEUE moved it to another key/bucket meanwhile.
pub struct WaitHandle {
    location: Arc<SpinLock<FutexKey>>,
}

fn wait_kind(
    key: FutexKey,
    word: &FutexWord,
    expected: i32,
    bitset: u32,
    kind: FutexWaitKind,
    name: &'static str,
) -> SysResult<WaitHandle> {
    let location = Arc::new(SpinLock::new(key, "futex_wait_location"));

    let mut bucket = BUCKETS[bucket_of(&key)].lock();
    if word.read() != expected {
        return Err(Errno::EAGAIN);
    }
    // May refuse when a signal is pending; the task then stays runnable with
    // Event::Signal stashed, schedule() returns immediately and the caller's
    // cancel path removes the entry we enqueue below.
    current::task().block(name);
    bucket.enqueue(
        key,
        FutexWaitQueueItem {
            tcb: current::task().clone(),
            bitset,
            kind,
            location: location.clone(),
        },
    );
    drop(bucket);

    Ok(WaitHandle { location })
}

/// FUTEX_WAIT: atomically (w.r.t. `wake` on the same bucket) re-checks the
/// futex word and queues the current task. The caller must have pinned the
/// word beforehand and calls `current::schedule()` afterwards.
pub fn wait(key: FutexKey, word: &FutexWord, expected: i32, bitset: u32) -> SysResult<WaitHandle> {
    wait_kind(key, word, expected, bitset, FutexWaitKind::Wait, "futex")
}

/// Value check under the bucket lock without enqueueing (zero-timeout wait).
pub fn check(key: FutexKey, word: &FutexWord, expected: i32) -> SysResult<()> {
    let _bucket = BUCKETS[bucket_of(&key)].lock();
    if word.read() != expected {
        return Err(Errno::EAGAIN);
    }
    Ok(())
}

/// FUTEX_WAKE: wakes up to `num` waiters on `key` whose bitset intersects
/// `mask`. Touches only the key's bucket; no user memory access.
pub fn wake(key: FutexKey, num: usize, mask: u32) -> SysResult<usize> {
    let mut bucket = BUCKETS[bucket_of(&key)].lock();
    let Some(futex) = bucket.futexes.get_mut(&key) else {
        return Ok(0);
    };
    let woken = futex.wake(num, mask);
    bucket.cleanup(&key);
    Ok(woken)
}

fn requeue_locked(
    src: &mut Bucket,
    dst: Option<&mut Bucket>,
    key: FutexKey,
    key2: FutexKey,
    wake_count: usize,
    requeue_count: usize,
    cmp: Option<(&FutexWord, i32)>,
) -> SysResult<usize> {
    if let Some((word, expected)) = cmp {
        if word.read() != expected {
            return Err(Errno::EAGAIN);
        }
    }

    let Some(futex) = src.futexes.get_mut(&key) else {
        return Ok(0);
    };
    let woken = futex.wake(wake_count, u32::MAX);

    let mut pending = LinkedList::new();
    let mut moved = 0;
    while moved < requeue_count {
        let Some(item) = futex.wait_list.pop_front() else {
            break;
        };
        pending.push_back(item);
        moved += 1;
    }

    if moved == 0 {
        src.cleanup(&key);
        return Ok(woken);
    }

    if key != key2 {
        src.cleanup(&key);
    }

    // Retag the moved waiters: both bucket locks are held, so cancel_wait
    // observes either the old key (and blocks on the old bucket lock) or the
    // new one.
    for item in pending.iter() {
        *item.location.lock() = key2;
    }

    let dst_bucket = match dst {
        Some(dst) => dst,
        None => src,
    };
    let futex2 = dst_bucket.futexes.entry(key2).or_insert_with(Futex::new);
    futex2.wait_list.append(&mut pending);

    Ok(woken + moved)
}

/// FUTEX_REQUEUE / FUTEX_CMP_REQUEUE: wakes up to `wake_count` waiters on
/// `key` and moves up to `requeue_count` of the remaining ones to `key2`.
/// With `cmp`, first re-checks the (pre-pinned) futex word under the locks.
/// When the two keys hash to different buckets, both bucket locks are taken
/// in ascending index order.
pub fn requeue(
    key: FutexKey,
    key2: FutexKey,
    wake_count: usize,
    requeue_count: usize,
    cmp: Option<(&FutexWord, i32)>,
) -> SysResult<usize> {
    let b1 = bucket_of(&key);
    let b2 = bucket_of(&key2);

    if b1 == b2 {
        let mut bucket = BUCKETS[b1].lock();
        requeue_locked(&mut bucket, None, key, key2, wake_count, requeue_count, cmp)
    } else {
        let (lo, hi) = if b1 < b2 { (b1, b2) } else { (b2, b1) };
        let mut guard_lo = BUCKETS[lo].lock();
        // SAFETY: bucket locks are always acquired in ascending index order,
        // so this acquisition cannot participate in a lock-order deadlock;
        // the unchecked variant only skips lockdep's same-name class check.
        let mut guard_hi = unsafe { BUCKETS[hi].lock_unchecked() };
        let (src, dst): (&mut Bucket, &mut Bucket) = if b1 == lo {
            (&mut guard_lo, &mut guard_hi)
        } else {
            (&mut guard_hi, &mut guard_lo)
        };
        requeue_locked(src, Some(dst), key, key2, wake_count, requeue_count, cmp)
    }
}

/// Entry for `waitv`: the key, the pre-pinned futex word and the expected value.
pub struct WaitvEntry {
    pub key: FutexKey,
    pub word: FutexWord,
    pub expected: i32,
}

/// FUTEX_WAITV: atomically checks every futex word and, when `enqueue` is
/// set, blocks the current task and links it into every queue.
///
/// All involved bucket locks (deduplicated, at most BUCKET_COUNT) are held
/// together, acquired in ascending index order. This preserves the guarantee
/// the old global lock provided against this lost-wakeup race:
///
///   | Task A (waiter)             | Task B (waker)
/// 0 | read futex == expected      |
/// 1 |                             | store futex = new_value
/// 2 |                             | futex_wake()
/// 3 | enqueue and sleep           |
///
/// A waker targeting any involved key serializes on that key's bucket lock
/// and therefore observes either "nothing queued yet" (before we re-read the
/// word) or the fully queued waiter.
pub fn waitv(entries: &[WaitvEntry], enqueue: bool) -> SysResult<Vec<WaitHandle>> {
    // Allocate the location cells before taking any bucket lock.
    let locations: Vec<Arc<SpinLock<FutexKey>>> = entries
        .iter()
        .map(|entry| Arc::new(SpinLock::new(entry.key, "futex_wait_location")))
        .collect();

    let mut indices: Vec<usize> = entries.iter().map(|entry| bucket_of(&entry.key)).collect();
    indices.sort_unstable();
    indices.dedup();

    let mut guards = Vec::with_capacity(indices.len());
    for (i, &bucket_index) in indices.iter().enumerate() {
        let guard = if i == 0 {
            BUCKETS[bucket_index].lock()
        } else {
            // SAFETY: ascending index order, see `requeue`.
            unsafe { BUCKETS[bucket_index].lock_unchecked() }
        };
        guards.push((bucket_index, guard));
    }

    for entry in entries {
        if entry.word.read() != entry.expected {
            return Err(Errno::EAGAIN);
        }
    }

    if !enqueue {
        return Ok(Vec::new());
    }

    // See the pending-signal note in `wait_kind`.
    current::task().block("futex_waitv");

    let mut handles = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let bucket_index = bucket_of(&entry.key);
        let (_, bucket) = guards
            .iter_mut()
            .find(|(index, _)| *index == bucket_index)
            .expect("bucket lock must have been acquired");
        let location = locations[index].clone();
        bucket.enqueue(
            entry.key,
            FutexWaitQueueItem {
                tcb: current::task().clone(),
                bitset: u32::MAX,
                kind: FutexWaitKind::Waitv { index },
                location: location.clone(),
            },
        );
        handles.push(WaitHandle { location });
    }

    Ok(handles)
}

/// Removes the waiter identified by `handle` (if still queued) after a
/// timeout/EINTR/spurious wakeup. Only the bucket currently holding the
/// waiter is touched. The retry loop chases concurrent requeues: `location`
/// is only rewritten under both bucket locks, so once it is re-verified under
/// the bucket lock it points into, the entry cannot move.
pub fn cancel_wait(task: &Arc<dyn Task>, handle: &WaitHandle) -> usize {
    loop {
        let key = *handle.location.lock();
        let mut bucket = BUCKETS[bucket_of(&key)].lock();
        if *handle.location.lock() != key {
            // Requeued while we were acquiring the bucket lock; retry.
            continue;
        }
        let Some(futex) = bucket.futexes.get_mut(&key) else {
            // Already woken and dequeued.
            return 0;
        };
        let removed = futex.remove_waiter(task);
        bucket.cleanup(&key);
        return removed;
    }
}
