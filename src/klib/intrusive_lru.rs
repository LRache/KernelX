use alloc::sync::{Arc, Weak};
use core::cell::UnsafeCell;
use core::num::NonZeroUsize;

use crate::klib::SpinLock;

struct IntrusiveLruLinks<T: ?Sized> {
    prev: Option<Weak<T>>,
    next: Option<Arc<T>>,
    bucket: Option<NonZeroUsize>,
}

/// Links embedded in an object managed by [`IntrusiveLru`].
///
/// A node may only be managed by one LRU instance. The containing object must
/// not be dropped while its node is linked; the LRU's strong references enforce
/// this when all membership changes go through [`IntrusiveLru`].
pub struct IntrusiveLruNode<T: ?Sized> {
    links: UnsafeCell<IntrusiveLruLinks<T>>,
}

impl<T: ?Sized> IntrusiveLruNode<T> {
    pub const fn new() -> Self {
        Self {
            links: UnsafeCell::new(IntrusiveLruLinks {
                prev: None,
                next: None,
                bucket: None,
            }),
        }
    }
}

impl<T: ?Sized> Default for IntrusiveLruNode<T> {
    fn default() -> Self {
        Self::new()
    }
}

pub trait IntrusiveLruNodeOps<T: ?Sized> {
    fn lru_node(&self) -> &IntrusiveLruNode<T>;
}

// SAFETY: Link access is serialized by the owning `IntrusiveLru`'s lock. A node
// is only ever managed by one LRU instance, as required by the node contract.
unsafe impl<T: ?Sized + Send + Sync> Sync for IntrusiveLruNode<T> {}

struct IntrusiveLruInner<T: ?Sized> {
    head: Option<Arc<T>>,
    tail: Option<Arc<T>>,
    len: usize,
}

impl<T: ?Sized + IntrusiveLruNodeOps<T>> IntrusiveLruInner<T> {
    fn links<'a>(node: &'a IntrusiveLruNode<T>) -> &'a mut IntrusiveLruLinks<T> {
        // SAFETY: `IntrusiveLruInner` is private and every caller holds the
        // owning LRU's lock. A node belongs to at most one LRU instance.
        unsafe { &mut *node.links.get() }
    }

    fn unlink(&mut self, item: &Arc<T>) -> bool {
        let node = item.lru_node();
        let links = Self::links(node);
        if links.bucket.is_none() {
            return false;
        }

        let prev = links.prev.take().and_then(|prev| prev.upgrade());
        let next = links.next.take();

        if let Some(prev) = &prev {
            Self::links(prev.lru_node()).next = next.clone();
        } else {
            self.head = next.clone();
        }

        if let Some(next) = &next {
            Self::links(next.lru_node()).prev = prev.as_ref().map(Arc::downgrade);
        } else {
            self.tail = prev;
        }

        links.bucket = None;
        self.len -= 1;
        true
    }

    fn link_front(&mut self, item: Arc<T>, bucket: usize) {
        let links = Self::links(item.lru_node());
        assert!(links.bucket.is_none(), "intrusive LRU node is already linked");
        debug_assert!(links.prev.is_none());
        debug_assert!(links.next.is_none());

        links.next = self.head.take();
        if let Some(old_head) = &links.next {
            Self::links(old_head.lru_node()).prev = Some(Arc::downgrade(&item));
        } else {
            self.tail = Some(item.clone());
        }

        links.bucket = NonZeroUsize::new(bucket.checked_add(1).expect("intrusive LRU bucket index overflow"));
        self.head = Some(item);
        self.len += 1;
    }
}

/// A generic intrusive LRU whose entries are kept alive while linked.
///
/// The head is the most recently inserted end and the tail is the
/// victim-selection end.
pub struct IntrusiveLru<T: ?Sized + IntrusiveLruNodeOps<T>> {
    inner: SpinLock<IntrusiveLruInner<T>>,
}

impl<T: ?Sized + IntrusiveLruNodeOps<T>> IntrusiveLru<T> {
    pub fn new(name: &'static str) -> Self {
        Self {
            inner: SpinLock::new(
                IntrusiveLruInner {
                    head: None,
                    tail: None,
                    len: 0,
                },
                name,
            ),
        }
    }

    /// Links an unlinked entry at the most-recently-used end.
    pub fn link_front(&self, item: Arc<T>) {
        let mut inner = self.inner.lock();
        inner.link_front(item, 0);
    }

    /// Unlinks `item`, returning whether it was linked.
    pub fn unlink(&self, item: &Arc<T>) -> bool {
        self.inner.lock().unlink(item)
    }

    /// Removes and returns the least-recently-used entry.
    pub fn pop_back(&self) -> Option<Arc<T>> {
        let mut inner = self.inner.lock();
        let item = inner.tail.clone()?;
        let unlinked = inner.unlink(&item);
        debug_assert!(unlinked);
        Some(item)
    }

    pub fn len(&self) -> usize {
        self.inner.lock().len
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

struct PriorityIntrusiveLruInner<T: ?Sized + IntrusiveLruNodeOps<T>, const N: usize> {
    buckets: [IntrusiveLruInner<T>; N],
}

pub struct PriorityIntrusiveLru<T: ?Sized + IntrusiveLruNodeOps<T>, const N: usize> {
    inner: SpinLock<PriorityIntrusiveLruInner<T, N>>,
}

impl<T: ?Sized + IntrusiveLruNodeOps<T>, const N: usize> PriorityIntrusiveLru<T, N> {
    pub fn new(name: &'static str) -> Self {
        Self {
            inner: SpinLock::new(
                PriorityIntrusiveLruInner {
                    buckets: core::array::from_fn(|_| IntrusiveLruInner {
                        head: None,
                        tail: None,
                        len: 0,
                    }),
                },
                name,
            ),
        }
    }

    pub fn link_front(&self, item: Arc<T>, bucket: usize) {
        assert!(bucket < N, "priority LRU bucket is out of range");
        self.inner.lock().buckets[bucket].link_front(item, bucket);
    }

    pub fn move_to_front(&self, item: Arc<T>, bucket: usize) {
        assert!(bucket < N, "priority LRU bucket is out of range");
        let mut inner = self.inner.lock();
        let links = IntrusiveLruInner::links(item.lru_node());
        if let Some(old_bucket) = links.bucket {
            inner.buckets[old_bucket.get() - 1].unlink(&item);
        }
        inner.buckets[bucket].link_front(item, bucket);
    }

    pub fn unlink(&self, item: &Arc<T>) -> bool {
        let mut inner = self.inner.lock();
        let links = IntrusiveLruInner::links(item.lru_node());
        let Some(bucket) = links.bucket else {
            return false;
        };
        inner.buckets[bucket.get() - 1].unlink(item)
    }

    pub fn pop_back(&self) -> Option<Arc<T>> {
        let mut inner = self.inner.lock();
        for bucket in &mut inner.buckets {
            let Some(item) = bucket.tail.clone() else {
                continue;
            };
            let unlinked = bucket.unlink(&item);
            debug_assert!(unlinked);
            return Some(item);
        }
        None
    }

    pub fn len(&self) -> usize {
        self.inner.lock().buckets.iter().map(|bucket| bucket.len).sum()
    }
}

impl<T: ?Sized + IntrusiveLruNodeOps<T>> Drop for IntrusiveLru<T> {
    fn drop(&mut self) {
        while self.pop_back().is_some() {}
    }
}

impl<T: ?Sized + IntrusiveLruNodeOps<T>, const N: usize> Drop for PriorityIntrusiveLru<T, N> {
    fn drop(&mut self) {
        while self.pop_back().is_some() {}
    }
}

#[cfg(test)]
mod tests {
    use alloc::sync::{Arc, Weak};

    use super::{IntrusiveLru, IntrusiveLruNode, IntrusiveLruNodeOps};

    struct Entry {
        value: usize,
        lru_node: IntrusiveLruNode<Entry>,
    }

    impl Entry {
        fn new(value: usize) -> Arc<Self> {
            Arc::new(Self {
                value,
                lru_node: IntrusiveLruNode::new(),
            })
        }
    }

    impl IntrusiveLruNodeOps<Entry> for Entry {
        fn lru_node(&self) -> &IntrusiveLruNode<Entry> {
            &self.lru_node
        }
    }

    trait ErasedEntry: Send + Sync {
        fn value(&self) -> usize;
        fn erased_lru_node(&self) -> &IntrusiveLruNode<dyn ErasedEntry>;
    }

    struct ConcreteErasedEntry {
        value: usize,
        lru_node: IntrusiveLruNode<dyn ErasedEntry>,
    }

    impl ErasedEntry for ConcreteErasedEntry {
        fn value(&self) -> usize {
            self.value
        }

        fn erased_lru_node(&self) -> &IntrusiveLruNode<dyn ErasedEntry> {
            &self.lru_node
        }
    }

    impl IntrusiveLruNodeOps<dyn ErasedEntry> for dyn ErasedEntry {
        fn lru_node(&self) -> &IntrusiveLruNode<dyn ErasedEntry> {
            self.erased_lru_node()
        }
    }

    #[test]
    fn pops_from_least_recently_linked_end() {
        let lru = IntrusiveLru::new("test::pops_from_least_recently_linked_end");
        let first = Entry::new(1);
        let second = Entry::new(2);
        let third = Entry::new(3);

        lru.link_front(first);
        lru.link_front(second);
        lru.link_front(third);

        assert_eq!(lru.len(), 3);
        assert_eq!(lru.pop_back().unwrap().value, 1);
        assert_eq!(lru.pop_back().unwrap().value, 2);
        assert_eq!(lru.pop_back().unwrap().value, 3);
        assert!(lru.pop_back().is_none());
        assert!(lru.is_empty());
    }

    #[test]
    fn unlinks_an_entry_from_the_middle() {
        let lru = IntrusiveLru::new("test::unlinks_an_entry_from_the_middle");
        let first = Entry::new(1);
        let middle = Entry::new(2);
        let last = Entry::new(3);

        lru.link_front(first);
        lru.link_front(middle.clone());
        lru.link_front(last);

        assert!(lru.unlink(&middle));
        assert!(!lru.unlink(&middle));
        assert_eq!(lru.len(), 2);
        assert_eq!(lru.pop_back().unwrap().value, 1);
        assert_eq!(lru.pop_back().unwrap().value, 3);
    }

    #[test]
    fn unlinked_entry_can_be_relinked() {
        let lru = IntrusiveLru::new("test::unlinked_entry_can_be_relinked");
        let entry = Entry::new(1);

        lru.link_front(entry.clone());
        assert!(lru.unlink(&entry));

        lru.link_front(entry.clone());
        assert!(Arc::ptr_eq(&lru.pop_back().unwrap(), &entry));
    }

    #[test]
    fn linked_entries_are_kept_alive_until_lru_drop() {
        let entry = Entry::new(1);
        let weak: Weak<Entry> = Arc::downgrade(&entry);
        let lru = IntrusiveLru::new("test::linked_entries_are_kept_alive_until_lru_drop");

        lru.link_front(entry);
        assert!(weak.upgrade().is_some());

        drop(lru);
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn supports_type_erased_entries() {
        let lru: IntrusiveLru<dyn ErasedEntry> = IntrusiveLru::new("test::supports_type_erased_entries");
        let entry: Arc<dyn ErasedEntry> = Arc::new(ConcreteErasedEntry {
            value: 1,
            lru_node: IntrusiveLruNode::new(),
        });

        lru.link_front(entry);
        assert_eq!(lru.pop_back().unwrap().value(), 1);
    }
}
