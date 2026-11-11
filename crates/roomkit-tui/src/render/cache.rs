//! A bounded cache with one entry per identity: a message or a tool call.
//! A changed source replaces its entry instead of adding one, so streaming a
//! long answer never evicts the rest of the history.

use std::collections::HashMap;

const BYTE_LIMIT: usize = 4 * 1024 * 1024;
const ENTRY_LIMIT: usize = 500;

pub(crate) struct Cache<V> {
    entries: HashMap<String, Entry<V>>,
    clock: u64,
    bytes: usize,
}

struct Entry<V> {
    value: V,
    used: u64,
    bytes: usize,
}

impl<V> Default for Cache<V> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            clock: 0,
            bytes: 0,
        }
    }
}

impl<V> Cache<V> {
    /// The entry for `key`, marked as recently used.
    pub(crate) fn get(&mut self, key: &str) -> Option<&V> {
        self.clock += 1;
        let entry = self.entries.get_mut(key)?;
        entry.used = self.clock;
        Some(&entry.value)
    }

    /// Replace the entry for `key`. Values larger than the cache are not kept.
    pub(crate) fn insert(&mut self, key: String, value: V, bytes: usize) {
        if let Some(old) = self.entries.remove(&key) {
            self.bytes -= old.bytes;
        }
        if bytes > BYTE_LIMIT {
            return;
        }
        while self.bytes + bytes > BYTE_LIMIT || self.entries.len() >= ENTRY_LIMIT {
            self.evict_oldest();
        }
        self.clock += 1;
        self.bytes += bytes;
        self.entries.insert(
            key,
            Entry {
                value,
                used: self.clock,
                bytes,
            },
        );
    }

    fn evict_oldest(&mut self) {
        let oldest = self
            .entries
            .iter()
            .min_by_key(|(_, entry)| entry.used)
            .map(|(key, _)| key.clone());
        if let Some(entry) = oldest.and_then(|key| self.entries.remove(&key)) {
            self.bytes -= entry.bytes;
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    pub(crate) fn bytes(&self) -> usize {
        self.bytes
    }
}

/// Approximate retained bytes of rendered lines.
pub(crate) fn line_bytes(lines: &[super::Line]) -> usize {
    std::mem::size_of_val(lines)
        + lines
            .iter()
            .flatten()
            .map(|span| {
                span.text.len()
                    + span.hyperlink.as_ref().map_or(0, |link| link.url().len())
                    + std::mem::size_of::<super::Span>()
            })
            .sum::<usize>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_changing_source_keeps_one_entry_and_others_survive() {
        let mut cache = Cache::default();
        cache.insert("history".into(), 1, 10);
        for revision in 0..2_000 {
            cache.insert("streaming".into(), revision, 10);
        }
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.get("history"), Some(&1));
        assert_eq!(cache.bytes(), 20);
    }

    #[test]
    fn limits_evict_the_least_recently_used_entry() {
        let mut cache = Cache::default();
        for index in 0..ENTRY_LIMIT {
            cache.insert(index.to_string(), index, 1);
        }
        cache.get("0");
        cache.insert("new".into(), 0, 1);
        assert_eq!(cache.len(), ENTRY_LIMIT);
        assert!(cache.get("0").is_some(), "Recently used entries stay");
        assert!(cache.get("1").is_none(), "The oldest unused entry goes");
        cache.insert("huge".into(), 0, BYTE_LIMIT + 1);
        assert!(cache.get("huge").is_none());
        cache.insert("half".into(), 0, BYTE_LIMIT / 2);
        cache.insert("other half".into(), 0, BYTE_LIMIT / 2 + 1);
        assert!(cache.bytes() <= BYTE_LIMIT);
    }
}
