//! Index-based doubly-linked list for LRU tracking.

use interfaces::CacheKey;

#[derive(Debug)]
struct Node {
    key: CacheKey,
    prev: Option<usize>,
    next: Option<usize>,
    active: bool,
}

/// An index-based doubly-linked list for O(1) LRU operations.
///
/// Nodes are stored in a `Vec` and referenced by index. Removed nodes
/// are recycled via a free list.
pub(crate) struct LruList {
    nodes: Vec<Node>,
    head: Option<usize>,
    tail: Option<usize>,
    free: Vec<usize>,
}

impl LruList {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            head: None,
            tail: None,
            free: Vec::new(),
        }
    }

    /// Insert a key at the back (most recently used). Returns the node index.
    pub fn push_back(&mut self, key: CacheKey) -> usize {
        let idx = if let Some(free_idx) = self.free.pop() {
            self.nodes[free_idx] = Node {
                key,
                prev: self.tail,
                next: None,
                active: true,
            };
            free_idx
        } else {
            let idx = self.nodes.len();
            self.nodes.push(Node {
                key,
                prev: self.tail,
                next: None,
                active: true,
            });
            idx
        };

        if let Some(old_tail) = self.tail {
            self.nodes[old_tail].next = Some(idx);
        }
        self.tail = Some(idx);
        if self.head.is_none() {
            self.head = Some(idx);
        }

        idx
    }

    /// Move an existing node to the back (most recently used).
    pub fn move_to_back(&mut self, idx: usize) {
        if self.tail == Some(idx) {
            return;
        }

        let prev = self.nodes[idx].prev;
        let next = self.nodes[idx].next;

        // Unlink from current position
        if let Some(p) = prev {
            self.nodes[p].next = next;
        } else {
            self.head = next;
        }
        if let Some(n) = next {
            self.nodes[n].prev = prev;
        }

        // Link at back
        self.nodes[idx].prev = self.tail;
        self.nodes[idx].next = None;
        if let Some(old_tail) = self.tail {
            self.nodes[old_tail].next = Some(idx);
        }
        self.tail = Some(idx);
    }

    /// Remove and return the front node (least recently used).
    pub fn pop_front(&mut self) -> Option<CacheKey> {
        let head_idx = self.head?;
        let key = self.nodes[head_idx].key;
        self.remove(head_idx);
        Some(key)
    }

    /// Return up to `n` keys starting from the front (oldest).
    pub fn peek_front_n(&self, n: usize) -> Vec<CacheKey> {
        let mut result = Vec::with_capacity(n);
        let mut current = self.head;
        while let Some(idx) = current {
            if result.len() >= n {
                break;
            }
            result.push(self.nodes[idx].key);
            current = self.nodes[idx].next;
        }
        result
    }

    /// Remove a node by index.
    pub fn remove(&mut self, idx: usize) {
        if !self.nodes[idx].active {
            return;
        }

        let prev = self.nodes[idx].prev;
        let next = self.nodes[idx].next;

        if let Some(p) = prev {
            self.nodes[p].next = next;
        } else {
            self.head = next;
        }
        if let Some(n) = next {
            self.nodes[n].prev = prev;
        } else {
            self.tail = prev;
        }

        self.nodes[idx].active = false;
        self.nodes[idx].prev = None;
        self.nodes[idx].next = None;
        self.free.push(idx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_and_pop() {
        let mut lru = LruList::new();
        lru.push_back(1);
        lru.push_back(2);
        lru.push_back(3);
        assert_eq!(lru.pop_front(), Some(1));
        assert_eq!(lru.pop_front(), Some(2));
        assert_eq!(lru.pop_front(), Some(3));
        assert_eq!(lru.pop_front(), None);
    }

    #[test]
    fn move_to_back_updates_order() {
        let mut lru = LruList::new();
        let a = lru.push_back(1);
        lru.push_back(2);
        lru.push_back(3);
        lru.move_to_back(a);
        assert_eq!(lru.pop_front(), Some(2));
        assert_eq!(lru.pop_front(), Some(3));
        assert_eq!(lru.pop_front(), Some(1));
    }

    #[test]
    fn move_tail_to_back_is_noop() {
        let mut lru = LruList::new();
        lru.push_back(1);
        lru.push_back(2);
        let c = lru.push_back(3);
        lru.move_to_back(c);
        assert_eq!(lru.pop_front(), Some(1));
        assert_eq!(lru.pop_front(), Some(2));
        assert_eq!(lru.pop_front(), Some(3));
    }

    #[test]
    fn remove_middle() {
        let mut lru = LruList::new();
        lru.push_back(1);
        let b = lru.push_back(2);
        lru.push_back(3);
        lru.remove(b);
        assert_eq!(lru.pop_front(), Some(1));
        assert_eq!(lru.pop_front(), Some(3));
        assert_eq!(lru.pop_front(), None);
    }

    #[test]
    fn remove_head() {
        let mut lru = LruList::new();
        let a = lru.push_back(1);
        lru.push_back(2);
        lru.remove(a);
        assert_eq!(lru.pop_front(), Some(2));
        assert_eq!(lru.pop_front(), None);
    }

    #[test]
    fn remove_tail() {
        let mut lru = LruList::new();
        lru.push_back(1);
        let b = lru.push_back(2);
        lru.remove(b);
        assert_eq!(lru.pop_front(), Some(1));
        assert_eq!(lru.pop_front(), None);
    }

    #[test]
    fn free_list_reuses_slots() {
        let mut lru = LruList::new();
        let a = lru.push_back(1);
        lru.push_back(2);
        lru.remove(a);
        let c = lru.push_back(3);
        // Slot 0 (was key=1) gets reused for key=3
        assert_eq!(c, 0);
        assert_eq!(lru.pop_front(), Some(2));
        assert_eq!(lru.pop_front(), Some(3));
    }

    #[test]
    fn single_element() {
        let mut lru = LruList::new();
        let a = lru.push_back(42);
        lru.move_to_back(a);
        assert_eq!(lru.pop_front(), Some(42));
        assert_eq!(lru.pop_front(), None);
    }
}
