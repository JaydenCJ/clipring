//! The clipboard history ring: ordering, deduplication, pinning, eviction.
//!
//! Pure data structure — no I/O, no clock. The newest entry is index 0.
//! Capacity applies to *unpinned* entries only: pinned entries survive any
//! amount of new traffic, which is the whole point of pinning.

/// One clipboard capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Stable identifier, monotonically increasing across the ring's life.
    pub id: u64,
    /// Capture time, milliseconds since the Unix epoch.
    pub at_ms: u64,
    /// Pinned entries are never evicted by capacity.
    pub pinned: bool,
    /// The raw clipboard bytes (not necessarily UTF-8).
    pub data: Vec<u8>,
}

impl Entry {
    /// The entry's content as text, if it is valid UTF-8.
    pub fn text(&self) -> Option<&str> {
        std::str::from_utf8(&self.data).ok()
    }
}

/// History ring; index 0 is the newest entry.
#[derive(Debug)]
pub struct Ring {
    entries: Vec<Entry>,
    capacity: usize,
    next_id: u64,
}

impl Ring {
    pub fn new(capacity: usize) -> Ring {
        Ring {
            entries: Vec::new(),
            capacity: capacity.max(1),
            next_id: 1,
        }
    }

    /// Rebuild a ring from stored entries (newest first, as saved).
    pub fn from_entries(entries: Vec<Entry>, capacity: usize) -> Ring {
        let next_id = entries.iter().map(|e| e.id).max().unwrap_or(0) + 1;
        let mut ring = Ring {
            entries,
            capacity: capacity.max(1),
            next_id,
        };
        ring.evict();
        ring
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn pinned_count(&self) -> usize {
        self.entries.iter().filter(|e| e.pinned).count()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter()
    }

    pub fn get(&self, index: usize) -> Option<&Entry> {
        self.entries.get(index)
    }

    /// Insert `data` at the front. If identical bytes already exist anywhere
    /// in the ring, that entry is *promoted* instead of duplicated — its id
    /// and pin state survive, its timestamp is refreshed. Returns the index
    /// of the entry (always 0) — callers use the returned reference.
    pub fn push(&mut self, data: Vec<u8>, now_ms: u64) -> &Entry {
        if let Some(pos) = self.entries.iter().position(|e| e.data == data) {
            let mut entry = self.entries.remove(pos);
            entry.at_ms = now_ms;
            self.entries.insert(0, entry);
        } else {
            let entry = Entry {
                id: self.next_id,
                at_ms: now_ms,
                pinned: false,
                data,
            };
            self.next_id += 1;
            self.entries.insert(0, entry);
            self.evict();
        }
        &self.entries[0]
    }

    /// Move the entry at `index` to the front, refreshing its timestamp.
    pub fn promote(&mut self, index: usize, now_ms: u64) -> Result<&Entry, String> {
        self.check(index)?;
        let mut entry = self.entries.remove(index);
        entry.at_ms = now_ms;
        self.entries.insert(0, entry);
        Ok(&self.entries[0])
    }

    /// Pin or unpin the entry at `index`.
    pub fn set_pinned(&mut self, index: usize, pinned: bool) -> Result<(), String> {
        self.check(index)?;
        self.entries[index].pinned = pinned;
        // Unpinning may push the unpinned population over capacity.
        if !pinned {
            self.evict();
        }
        Ok(())
    }

    /// Remove and return the entry at `index`.
    pub fn remove(&mut self, index: usize) -> Result<Entry, String> {
        self.check(index)?;
        Ok(self.entries.remove(index))
    }

    /// Remove unpinned entries (or everything with `all`). Returns the count.
    pub fn clear(&mut self, all: bool) -> usize {
        let before = self.entries.len();
        if all {
            self.entries.clear();
        } else {
            self.entries.retain(|e| e.pinned);
        }
        before - self.entries.len()
    }

    /// Indices of text entries containing `needle`, case-insensitively.
    pub fn search(&self, needle: &str) -> Vec<usize> {
        let needle = needle.to_lowercase();
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.text().is_some_and(|t| t.to_lowercase().contains(&needle)))
            .map(|(i, _)| i)
            .collect()
    }

    fn check(&self, index: usize) -> Result<(), String> {
        if index >= self.entries.len() {
            return Err(format!(
                "no entry {index} (history has {} entr{})",
                self.entries.len(),
                if self.entries.len() == 1 { "y" } else { "ies" }
            ));
        }
        Ok(())
    }

    /// Drop the oldest unpinned entries until at most `capacity` remain.
    fn evict(&mut self) {
        let mut unpinned = self.entries.iter().filter(|e| !e.pinned).count();
        while unpinned > self.capacity {
            let last = self
                .entries
                .iter()
                .rposition(|e| !e.pinned)
                .expect("unpinned > 0 implies one exists");
            self.entries.remove(last);
            unpinned -= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ring_with(items: &[&str]) -> Ring {
        let mut ring = Ring::new(50);
        for (i, s) in items.iter().enumerate() {
            ring.push(s.as_bytes().to_vec(), i as u64 * 1000);
        }
        ring
    }

    #[test]
    fn newest_entry_is_index_zero() {
        let ring = ring_with(&["first", "second", "third"]);
        assert_eq!(ring.get(0).unwrap().data, b"third");
        assert_eq!(ring.get(2).unwrap().data, b"first");
    }

    #[test]
    fn ids_are_stable_and_monotonic() {
        let ring = ring_with(&["a", "b", "c"]);
        let ids: Vec<u64> = ring.iter().map(|e| e.id).collect();
        assert_eq!(ids, [3, 2, 1]);
    }

    #[test]
    fn duplicate_push_promotes_instead_of_duplicating() {
        let mut ring = ring_with(&["a", "b", "c"]);
        ring.set_pinned(2, true).unwrap(); // pin "a"
        ring.push(b"a".to_vec(), 9000);
        assert_eq!(ring.len(), 3, "no duplicate entry created");
        assert_eq!(ring.get(0).unwrap().data, b"a");
        assert_eq!(ring.get(0).unwrap().id, 1, "identity survives promotion");
        assert_eq!(ring.get(0).unwrap().at_ms, 9000, "timestamp refreshed");
        assert!(ring.get(0).unwrap().pinned, "pin state survives promotion");
    }

    #[test]
    fn capacity_evicts_oldest_unpinned() {
        let mut ring = Ring::new(3);
        for (i, s) in ["a", "b", "c", "d"].iter().enumerate() {
            ring.push(s.as_bytes().to_vec(), i as u64);
        }
        assert_eq!(ring.len(), 3);
        assert!(ring.iter().all(|e| e.data != b"a"), "oldest was evicted");
    }

    #[test]
    fn pinned_entries_survive_eviction() {
        let mut ring = Ring::new(2);
        ring.push(b"precious".to_vec(), 0);
        ring.set_pinned(0, true).unwrap();
        for i in 0..10u64 {
            ring.push(format!("noise-{i}").into_bytes(), i + 1);
        }
        assert!(
            ring.iter().any(|e| e.data == b"precious"),
            "pinned entry must never be evicted"
        );
        // Capacity bounds the unpinned population, not the pinned one.
        assert_eq!(ring.len(), 3);
    }

    #[test]
    fn unpinning_reapplies_capacity() {
        let mut ring = Ring::new(1);
        ring.push(b"old".to_vec(), 0);
        ring.set_pinned(0, true).unwrap();
        ring.push(b"new".to_vec(), 1);
        assert_eq!(ring.len(), 2);
        ring.set_pinned(1, false).unwrap(); // "old" loses protection
        assert_eq!(ring.len(), 1);
        assert_eq!(ring.get(0).unwrap().data, b"new");
    }

    #[test]
    fn promote_moves_entry_to_front() {
        let mut ring = ring_with(&["a", "b", "c"]);
        ring.promote(2, 9000).unwrap();
        assert_eq!(ring.get(0).unwrap().data, b"a");
        assert_eq!(ring.len(), 3);
    }

    #[test]
    fn remove_and_clear_behave() {
        let mut ring = ring_with(&["a", "b", "c"]);
        ring.set_pinned(2, true).unwrap(); // pin "a"
        let removed = ring.remove(0).unwrap();
        assert_eq!(removed.data, b"c");
        assert_eq!(ring.clear(false), 1, "clear removes only unpinned");
        assert_eq!(ring.len(), 1);
        assert_eq!(ring.clear(true), 1, "clear --all removes pinned too");
        assert!(ring.is_empty());
    }

    #[test]
    fn out_of_range_index_is_a_useful_error() {
        let mut ring = ring_with(&["only"]);
        let err = ring.remove(5).unwrap_err();
        assert!(err.contains("no entry 5"), "err was: {err}");
        assert!(err.contains("1 entry"), "err was: {err}");
    }

    #[test]
    fn search_is_case_insensitive_and_ordered() {
        let ring = ring_with(&["SELECT * FROM users", "plain", "select 1"]);
        assert_eq!(ring.search("select"), vec![0, 2]);
        assert!(ring.search("missing").is_empty());
    }

    #[test]
    fn search_skips_binary_entries() {
        let mut ring = Ring::new(10);
        ring.push(vec![0xff, 0xfe, b'a'], 0);
        ring.push(b"a text".to_vec(), 1);
        assert_eq!(ring.search("a"), vec![0]);
    }

    #[test]
    fn from_entries_resumes_ids_and_enforces_capacity() {
        // Reload must never repeat an id, and shrinking CLIPRING_CAPACITY
        // must trim on the next load rather than panic.
        let stored = vec![
            Entry {
                id: 9,
                at_ms: 2,
                pinned: false,
                data: b"new".to_vec(),
            },
            Entry {
                id: 4,
                at_ms: 1,
                pinned: true,
                data: b"old".to_vec(),
            },
        ];
        let mut ring = Ring::from_entries(stored, 50);
        ring.push(b"next".to_vec(), 3);
        assert_eq!(ring.get(0).unwrap().id, 10, "ids never repeat after reload");

        let stored: Vec<Entry> = (0..5)
            .map(|i| Entry {
                id: 5 - i,
                at_ms: 0,
                pinned: false,
                data: vec![i as u8],
            })
            .collect();
        assert_eq!(Ring::from_entries(stored, 2).len(), 2);
    }
}
