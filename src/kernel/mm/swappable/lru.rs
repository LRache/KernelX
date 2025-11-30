use alloc::collections::BTreeMap;
use alloc::sync::{Arc, Weak};

type Link<K, V> = Option<Arc<Node<K, V>>>;

#[derive(Debug)]
pub struct Node<K, V> {
    pub key: K,
    pub value: V,
    prev: Weak<Node<K, V>>,
    next: Link<K, V>,
}

impl<K, V> Node<K, V> {
    pub fn new(key: K, value: V) -> Arc<Self> {
        Arc::new(Self {
            key,
            value,
            prev: Weak::new(),
            next: None,
        })
    }
}

#[derive(Debug)]
pub struct LruList<K, V> {
    head: Link<K, V>,
    tail: Link<K, V>,
}

impl<K, V> LruList<K, V> {
    pub fn new() -> Self {
        Self { head: None, tail: None }
    }

    pub fn push_front_node(&mut self, node: Arc<Node<K, V>>) {
        unsafe {
            let n = &mut *(Arc::as_ptr(&node) as *mut Node<K, V>);
            n.prev = Weak::new();
            n.next = None;
        }

        match self.head.take() {
            Some(old_head) => {
                unsafe {
                    let oh = &mut *(Arc::as_ptr(&old_head) as *mut Node<K, V>);
                    oh.prev = Arc::downgrade(&node);
                }
                unsafe {
                    let n = &mut *(Arc::as_ptr(&node) as *mut Node<K, V>);
                    n.next = Some(old_head.clone());
                }
                self.head = Some(node);
                if self.tail.is_none() {
                    self.tail = Some(old_head);
                }
            }
            None => {
                self.tail = Some(node.clone());
                self.head = Some(node);
            }
        }
    }

    pub fn move_to_front(&mut self, node: Arc<Node<K, V>>) {
        if let Some(head) = &self.head {
            if Arc::ptr_eq(head, &node) {
                return;
            }
        }

        let (prev, next) = unsafe {
            let n = &mut *(Arc::as_ptr(&node) as *mut Node<K, V>);
            (n.prev.upgrade(), n.next.clone())
        };

        if let Some(prev_node) = &prev {
            unsafe {
                let p = &mut *(Arc::as_ptr(prev_node) as *mut Node<K, V>);
                p.next = next.clone();
            }
        }

        if let Some(next_node) = &next {
            unsafe {
                let nx = &mut *(Arc::as_ptr(&next_node) as *mut Node<K, V>);
                nx.prev = node.prev.clone();
            }
        }

        if let Some(tail) = &self.tail {
            if Arc::ptr_eq(tail, &node) {
                self.tail = prev;
            }
        }

        self.push_front_node(node);
    }

    pub fn pop_back(&mut self) -> Option<Arc<Node<K, V>>> {
        self.tail.take().map(|old_tail| {
            let prev = unsafe {
                let t = &mut *(Arc::as_ptr(&old_tail) as *mut Node<K, V>);
                t.prev.upgrade()
            };

            match prev {
                Some(prev_node) => unsafe {
                    let p = &mut *(Arc::as_ptr(&prev_node) as *mut Node<K, V>);
                    p.next = None;
                    self.tail = Some(prev_node);
                },
                None => {
                    self.head = None;
                }
            }

            old_tail
        })
    }
}

pub struct LRUCache<K: Ord + Copy, V> {
    list: LruList<K, V>,
    map: BTreeMap<K, Arc<Node<K, V>>>,
}

impl<K: Ord + Copy, V> LRUCache<K, V> {
    pub fn new() -> Self {
        Self {
            list: LruList::new(),
            map: BTreeMap::new(),
        }
    }

    pub fn put(&mut self, key: K, value: V) {
        if let Some(node) = self.map.get(&key) {
            unsafe {
                let n = &mut *(Arc::as_ptr(node) as *mut Node<K, V>);
                n.value = value;
            }
            self.list.move_to_front(node.clone());
            return;
        }

        let new_node = Node::new(key, value);
        self.list.push_front_node(new_node.clone());
        self.map.insert(key, new_node);
    }

    pub fn access(&mut self, key: &K) -> bool {
        if let Some(node) = self.map.get(key) {
            self.list.move_to_front(node.clone());
            true
        } else {
            false
        }
    }

    pub fn pop_lru(&mut self) -> Option<K>
    where
        V: Clone,
    {
        if let Some(lru_node) = self.list.pop_back() {
            self.map.remove(&lru_node.key);
            Some(lru_node.key)
        } else {
            None    
        }
    }

    pub fn tail(&self) -> Option<(K, &V)> {
        self.list.tail.as_ref().map(|node| (node.key, &node.value))
    }

    pub fn remove(&mut self, key: &K) -> bool {
        if let Some(node) = self.map.remove(key) {
            let (prev, next) = unsafe {
                let n = &mut *(Arc::as_ptr(&node) as *mut Node<K, V>);
                (n.prev.upgrade(), n.next.clone())
            };

            if let Some(prev_node) = &prev {
                unsafe {
                    let p = &mut *(Arc::as_ptr(prev_node) as *mut Node<K, V>);
                    p.next = next.clone();
                }
            }

            if let Some(next_node) = &next {
                unsafe {
                    let nx = &mut *(Arc::as_ptr(&next_node) as *mut Node<K, V>);
                    nx.prev = node.prev.clone();
                }
            }

            if let Some(tail) = &self.list.tail {
                if Arc::ptr_eq(tail, &node) {
                    self.list.tail = prev;
                }
            }

            if let Some(head) = &self.list.head {
                if Arc::ptr_eq(head, &node) {
                    self.list.head = next;
                }
            }

            true
        } else {
            false
        }
    }
}
