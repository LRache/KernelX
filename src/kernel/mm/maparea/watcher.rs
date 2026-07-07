use alloc::sync::Arc;

use crate::kernel::mm::MapPerm;

#[derive(Clone, Copy, Debug)]
pub enum MapChangeEvent {
    Unmap,
    Remap,
    PermChange(MapPerm),
}

#[derive(Clone, Copy, Debug)]
pub struct MapChange {
    pub uaddr: usize,
    pub page_count: usize,
    pub event: MapChangeEvent,
}

pub trait MapManagerWatcher: Send + Sync {
    fn before_map_change(&self, change: MapChange);
}

pub struct MapChangeNotifier<'a> {
    watchers: &'a [Arc<dyn MapManagerWatcher>],
}

impl<'a> MapChangeNotifier<'a> {
    pub(super) fn new(watchers: &'a [Arc<dyn MapManagerWatcher>]) -> Self {
        Self { watchers }
    }

    pub fn before_map_change(&self, change: MapChange) {
        for watcher in self.watchers {
            watcher.before_map_change(change);
        }
    }
}
