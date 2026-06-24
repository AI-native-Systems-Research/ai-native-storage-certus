//! Index-based doubly-linked list for O(1) LRU operations.

use interfaces::CacheKey;

#[derive(Debug)]
struct Node {
    key: CacheKey,
    prev: Option<u32>,
    next: Option<u32>,
    active: bool,
}

/// An index-based doubly-linked list for O(1) LRU operations.
///
/// Nodes are stored in a `Vec` and referenced by index. Removed nodes
/// are recycled via a free list.
pub(crate) struct LruList {
    nodes: Vec<Node>,
    head: Option<u32>,
    tail: Option<u32>,
    free: Vec<u32>,
    len: usize,
}

impl LruList {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            head: None,
            tail: None,
            free: Vec::new(),
            len: 0,
        }
    }

    /// Insert a key at the back (most recently used). Returns the node index.
    pub fn push_back(&mut self, key: CacheKey) -> u32 {
        let idx = if let Some(free_idx) = self.free.pop() {
            self.nodes[free_idx as usize] = Node {
                key,
                prev: self.tail,
                next: None,
                active: true,
            };
            free_idx
        } else {
            let idx = self.nodes.len() as u32;
            self.nodes.push(Node {
                key,
                prev: self.tail,
                next: None,
                active: true,
            });
            idx
        };

        if let Some(old_tail) = self.tail {
            self.nodes[old_tail as usize].next = Some(idx);
        }
        self.tail = Some(idx);
        if self.head.is_none() {
            self.head = Some(idx);
        }

        self.len += 1;
        idx
    }

    /// Move an existing node to the back (most recently used).
    pub fn move_to_back(&mut self, idx: u32) {
        if !self.nodes[idx as usize].active {
            return;
        }
        if self.tail == Some(idx) {
            return;
        }

        let prev = self.nodes[idx as usize].prev;
        let next = self.nodes[idx as usize].next;

        if let Some(p) = prev {
            self.nodes[p as usize].next = next;
        } else {
            self.head = next;
        }
        if let Some(n) = next {
            self.nodes[n as usize].prev = prev;
        }

        self.nodes[idx as usize].prev = self.tail;
        self.nodes[idx as usize].next = None;
        if let Some(old_tail) = self.tail {
            self.nodes[old_tail as usize].next = Some(idx);
        }
        self.tail = Some(idx);
    }

    /// Remove and return the front node (least recently used).
    pub fn pop_front(&mut self) -> Option<CacheKey> {
        let head_idx = self.head?;
        let key = self.nodes[head_idx as usize].key;
        self.remove(head_idx);
        Some(key)
    }

    /// Return up to `n` keys starting from the front (oldest).
    pub fn peek_front_n(&self, n: usize) -> Vec<CacheKey> {
        let mut result = Vec::with_capacity(n.min(self.len as usize));
        let mut current = self.head;
        while let Some(idx) = current {
            if result.len() >= n {
                break;
            }
            result.push(self.nodes[idx as usize].key);
            current = self.nodes[idx as usize].next;
        }
        result
    }

    /// Remove a node by index. Idempotent for already-removed nodes.
    pub fn remove(&mut self, idx: u32) {
        if !self.nodes[idx as usize].active {
            return;
        }

        let prev = self.nodes[idx as usize].prev;
        let next = self.nodes[idx as usize].next;

        if let Some(p) = prev {
            self.nodes[p as usize].next = next;
        } else {
            self.head = next;
        }
        if let Some(n) = next {
            self.nodes[n as usize].prev = prev;
        } else {
            self.tail = prev;
        }

        self.nodes[idx as usize].active = false;
        self.nodes[idx as usize].prev = None;
        self.nodes[idx as usize].next = None;
        self.free.push(idx);
        self.len -= 1;
    }

    /// Reset the list to empty.
    pub fn clear(&mut self) {
        self.nodes.clear();
        self.head = None;
        self.tail = None;
        self.free.clear();
        self.len = 0;
    }

    /// Return the number of active entries.
    pub fn len(&self) -> usize {
        self.len
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

    #[test]
    fn len_tracks_active_entries() {
        let mut lru = LruList::new();
        assert_eq!(lru.len(), 0);
        lru.push_back(1);
        lru.push_back(2);
        assert_eq!(lru.len(), 2);
        lru.pop_front();
        assert_eq!(lru.len(), 1);
    }

    #[test]
    fn clear_resets_to_empty() {
        let mut lru = LruList::new();
        lru.push_back(1);
        lru.push_back(2);
        lru.push_back(3);
        lru.clear();
        assert_eq!(lru.len(), 0);
        assert_eq!(lru.pop_front(), None);
        // Can still insert after clear
        lru.push_back(10);
        assert_eq!(lru.pop_front(), Some(10));
    }

    #[test]
    fn remove_inactive_is_idempotent() {
        let mut lru = LruList::new();
        let a = lru.push_back(1);
        lru.remove(a);
        lru.remove(a); // no panic
        assert_eq!(lru.len(), 0);
    }

    #[test]
    fn move_to_back_inactive_is_noop() {
        let mut lru = LruList::new();
        let a = lru.push_back(1);
        lru.push_back(2);
        lru.remove(a);
        lru.move_to_back(a); // no panic, no effect
        assert_eq!(lru.pop_front(), Some(2));
    }

    #[test]
    fn peek_front_n() {
        let mut lru = LruList::new();
        lru.push_back(10);
        lru.push_back(20);
        lru.push_back(30);
        assert_eq!(lru.peek_front_n(2), vec![10, 20]);
        assert_eq!(lru.peek_front_n(5), vec![10, 20, 30]);
        assert_eq!(lru.peek_front_n(0), Vec::<CacheKey>::new());
    }
}

// Kani harness: peek_front_n must not panic for any value of n,
// including usize::MAX (the value passed by the eviction path to mean
// "return all candidates"). Without the cap, Vec::with_capacity(usize::MAX)
// panics unconditionally (capacity overflow). Brian Hatfield (PR #270)
// fixed this with n.min(self.len). This harness regresses it.
#[cfg(kani)]
mod verification {
    use super::*;

    #[kani::proof]
    #[kani::unwind(4)]
    fn verify_peek_front_n_no_panic() {
        let n: usize = kani::any();  // Kani will try usize::MAX among all values

        let mut lru = LruList::new();
        // Populate with a small number of entries (bounded by unwind limit)
        let num_entries: u8 = kani::any();
        kani::assume(num_entries <= 3);
        for i in 0..num_entries as u64 {
            lru.push_back(i);
        }

        // Must not panic for any n, including usize::MAX
        let result = lru.peek_front_n(n);

        // Result must never exceed the number of entries in the list
        assert!(result.len() <= num_entries as usize);
        // Result must never exceed the requested n
        assert!(result.len() <= n);
    }
}
