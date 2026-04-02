use alloc::collections::BTreeMap;
use alloc::collections::LinkedList;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use spin::mutex::SpinMutex;

/// Per-task lockdep state: tracks which locks the task currently holds.
///
/// Embed one instance in each task type (TCB, KThread). Safe to access without
/// synchronisation because a task runs on exactly one CPU at a time.
pub struct LockState {
    held: UnsafeCell<Vec<&'static str>>,
    waiting: SpinMutex<Option<(&'static str, LinkedList<usize>)>>,
}

impl LockState {
    pub const fn new() -> Self {
        Self { 
            held: UnsafeCell::new(Vec::new()),
            waiting: SpinMutex::new(None)
        }
    }

    pub fn acquire(&self, name: &'static str) {
        unsafe { (*self.held.get()).push(name) };
    }

    pub fn release(&self, name: &'static str) {
        let held = unsafe { &mut *self.held.get() };
        // LIFO removal — handles re-entrant same-name locks correctly.
        if let Some(pos) = held.iter().rposition(|&n| core::ptr::eq(n, name)) {
            held.remove(pos);
        } else {
            panic!("lockdep: releasing lock '{}' that was not held!", name);
        }
    }

    pub fn held(&self) -> &[&'static str] {
        unsafe { (*self.held.get()).as_slice() }
    }

    pub fn set_waiting(&self, waiting: Option<(&'static str, LinkedList<usize>)>) {
        *self.waiting.lock() = waiting;
    }

    pub fn waiting(&self) -> Option<(&'static str, LinkedList<usize>)> {
        self.waiting.lock().clone()
    }
}

unsafe impl Send for LockState {}
unsafe impl Sync for LockState {}


/// One directed edge in the lock dependency graph.
struct Edge {
    /// The name of the lock at the head of the edge (acquired after the key lock).
    to: &'static str,
    #[cfg(feature = "backtrace")]
    /// Backtrace captured at the moment this dependency was first observed.
    backtrace: LinkedList<usize>,
}

/// Dependency graph: class_id(A) → edges to locks acquired while A is held.
/// class_id is name.as_ptr() as usize (stable for &'static str literals).
static LOCK_DEP_GRAPH: SpinMutex<BTreeMap<usize, Vec<Edge>>> = SpinMutex::new(BTreeMap::new());

/// DFS reachability: can we get from `from_id` to `target_id` following existing graph edges?
/// The graph is always a DAG (we check before adding each edge), so this always terminates.
fn can_reach(graph: &BTreeMap<usize, Vec<Edge>>, from_id: usize, target_id: usize) -> bool {
    if from_id == target_id {
        return true;
    }
    if let Some(edges) = graph.get(&from_id) {
        for edge in edges {
            let dep_id = edge.to.as_ptr() as usize;
            if can_reach(graph, dep_id, target_id) {
                return true;
            }
        }
    }
    false
}

/// Find the backtrace of the first edge on the path from `from_id` to `target_id`.
/// Called only after `can_reach` confirmed a path exists.
fn find_path_backtrace<'a>(
    graph: &'a BTreeMap<usize, Vec<Edge>>,
    from_id: usize,
    target_id: usize,
) -> Option<&'a LinkedList<usize>> {
    if let Some(edges) = graph.get(&from_id) {
        for edge in edges {
            let dep_id = edge.to.as_ptr() as usize;
            if dep_id == target_id || can_reach(graph, dep_id, target_id) {
                return Some(&edge.backtrace);
            }
        }
    }
    None
}

/// Called just before acquiring `new_name` while `held` lists currently held locks.
///
/// For each held lock H, records the dependency H → new_name in the graph.
/// Panics if doing so would introduce a cycle (i.e. new_name can already reach H).
#[cfg(feature = "deadlock-detect")]
pub fn on_acquire(helds: &[&'static str], new_name: &'static str) {
    let new_id = new_name.as_ptr() as usize;
    
    if helds.iter().find(|&&held| held.as_ptr() as usize == new_id).is_some() {
        panic!("lockdep: lock order violation! Attempting to acquire '{}' while already holding it, helds={:?}", new_name, helds);
    }
    
    let mut graph = LOCK_DEP_GRAPH.lock();

    for &held in helds {
        let held_id = held.as_ptr() as usize;
        if held_id == new_id {
            continue;
        }
        // Would adding the edge held_id → new_name create a cycle?
        // A cycle exists iff new_id can already reach held_id.
        if can_reach(&graph, new_id, held_id) {
            crate::println!(
                "lockdep: lock order violation!\n  \
                 Acquiring '{}' while holding '{}',\n  \
                 but the reverse dependency '{}' -> ... -> '{}' was already recorded.",
                new_name, held, new_name, held
            );
            #[cfg(feature = "backtrace")]
            {
                use crate::klib::backtrace::print_backtrace_chain;
                if let Some(bt) = find_path_backtrace(&graph, new_id, held_id) {
                    print_backtrace_chain(bt);
                } else {
                    crate::println!("(No backtrace recorded for the reverse dependency.)");
                }
            }
            panic!("lockdep: lock order violation detected (see above)");
        }
        // Safe to record: held_id → new_name. Capture the current backtrace.
        let edges = graph.entry(held_id).or_default();
        if !edges.iter().any(|e| core::ptr::eq(e.to, new_name)) {
            edges.push(Edge {
                to: new_name,
                #[cfg(feature = "backtrace")]
                backtrace: crate::klib::backtrace::backtrace_chain(),
            });
        }
    }
}
