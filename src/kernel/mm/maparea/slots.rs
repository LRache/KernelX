use alloc::collections::{BTreeMap, btree_map};
use alloc::vec::Vec;
use core::slice;

/// Stores materialized area pages densely while their slot table fits in one
/// page, and sparsely for larger lazy mappings. A missing slot is unallocated.
pub(super) enum AreaPageSlots<T> {
    Dense(Vec<Option<T>>),
    Sparse {
        page_count: usize,
        key_base: usize,
        slots: BTreeMap<usize, T>,
    },
}

impl<T> AreaPageSlots<T> {
    pub(super) fn new(page_count: usize) -> Self {
        if Self::uses_dense_storage(page_count) {
            let mut slots = Vec::with_capacity(page_count);
            slots.resize_with(page_count, || None);
            Self::Dense(slots)
        } else {
            Self::Sparse {
                page_count,
                key_base: 0,
                slots: BTreeMap::new(),
            }
        }
    }

    pub(super) fn len(&self) -> usize {
        match self {
            Self::Dense(slots) => slots.len(),
            Self::Sparse { page_count, .. } => *page_count,
        }
    }

    pub(super) fn get(&self, index: usize) -> Option<&T> {
        if index >= self.len() {
            return None;
        }
        match self {
            Self::Dense(slots) => slots[index].as_ref(),
            Self::Sparse { key_base, slots, .. } => slots.get(&(key_base + index)),
        }
    }

    pub(super) fn insert(&mut self, index: usize, value: T) -> Option<T> {
        debug_assert!(index < self.len(), "page slot index out of bounds");
        match self {
            Self::Dense(slots) => slots[index].replace(value),
            Self::Sparse { key_base, slots, .. } => slots.insert(*key_base + index, value),
        }
    }

    pub(super) fn remove(&mut self, index: usize) -> Option<T> {
        if index >= self.len() {
            return None;
        }
        match self {
            Self::Dense(slots) => slots[index].take(),
            Self::Sparse { key_base, slots, .. } => slots.remove(&(*key_base + index)),
        }
    }

    pub(super) fn iter(&self) -> AreaPageSlotsIter<'_, T> {
        match self {
            Self::Dense(slots) => AreaPageSlotsIter::Dense(slots.iter().enumerate()),
            Self::Sparse { key_base, slots, .. } => AreaPageSlotsIter::Sparse {
                key_base: *key_base,
                iter: slots.iter(),
            },
        }
    }

    pub(super) fn iter_mut(&mut self) -> AreaPageSlotsIterMut<'_, T> {
        match self {
            Self::Dense(slots) => AreaPageSlotsIterMut::Dense(slots.iter_mut().enumerate()),
            Self::Sparse { key_base, slots, .. } => AreaPageSlotsIterMut::Sparse {
                key_base: *key_base,
                iter: slots.iter_mut(),
            },
        }
    }

    pub(super) fn split_off(&mut self, index: usize) -> Self {
        debug_assert!(index <= self.len(), "page slot split index out of bounds");
        let right = match self {
            Self::Dense(slots) => Self::Dense(slots.split_off(index)),
            Self::Sparse {
                page_count,
                key_base,
                slots,
            } => {
                let right_page_count = *page_count - index;
                let right_key_base = *key_base + index;
                let right_slots = slots.split_off(&right_key_base);
                *page_count = index;
                Self::from_sparse(right_page_count, right_key_base, right_slots)
            }
        };
        self.make_dense_if_small();
        right
    }

    pub(super) fn clear_with(&mut self, mut drop_slot: impl FnMut(usize, T)) {
        match self {
            Self::Dense(slots) => {
                for (index, slot) in slots.iter_mut().enumerate() {
                    if let Some(value) = slot.take() {
                        drop_slot(index, value);
                    }
                }
            }
            Self::Sparse { key_base, slots, .. } => {
                let key_base = *key_base;
                for (key, value) in core::mem::take(slots) {
                    drop_slot(key - key_base, value);
                }
            }
        }
    }

    fn from_sparse(page_count: usize, key_base: usize, slots: BTreeMap<usize, T>) -> Self {
        if Self::uses_dense_storage(page_count) {
            let mut dense = Vec::with_capacity(page_count);
            dense.resize_with(page_count, || None);
            for (key, value) in slots {
                dense[key - key_base] = Some(value);
            }
            Self::Dense(dense)
        } else {
            Self::Sparse {
                page_count,
                key_base,
                slots,
            }
        }
    }

    fn make_dense_if_small(&mut self) {
        let Self::Sparse { page_count, .. } = self else {
            return;
        };
        if !Self::uses_dense_storage(*page_count) {
            return;
        }

        let Self::Sparse {
            page_count,
            key_base,
            slots,
        } = core::mem::replace(self, Self::Dense(Vec::new()))
        else {
            unreachable!();
        };
        *self = Self::from_sparse(page_count, key_base, slots);
    }

    fn uses_dense_storage(page_count: usize) -> bool {
        page_count.saturating_mul(core::mem::size_of::<Option<T>>()) <= crate::arch::PGSIZE
    }
}

pub(super) enum AreaPageSlotsIter<'a, T> {
    Dense(core::iter::Enumerate<slice::Iter<'a, Option<T>>>),
    Sparse {
        key_base: usize,
        iter: btree_map::Iter<'a, usize, T>,
    },
}

impl<'a, T> Iterator for AreaPageSlotsIter<'a, T> {
    type Item = (usize, &'a T);

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Dense(iter) => iter.find_map(|(index, slot)| slot.as_ref().map(|value| (index, value))),
            Self::Sparse { key_base, iter } => iter.next().map(|(&key, value)| (key - *key_base, value)),
        }
    }
}

pub(super) enum AreaPageSlotsIterMut<'a, T> {
    Dense(core::iter::Enumerate<slice::IterMut<'a, Option<T>>>),
    Sparse {
        key_base: usize,
        iter: btree_map::IterMut<'a, usize, T>,
    },
}

impl<'a, T> Iterator for AreaPageSlotsIterMut<'a, T> {
    type Item = (usize, &'a mut T);

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Dense(iter) => iter.find_map(|(index, slot)| slot.as_mut().map(|value| (index, value))),
            Self::Sparse { key_base, iter } => iter.next().map(|(&key, value)| (key - *key_base, value)),
        }
    }
}
