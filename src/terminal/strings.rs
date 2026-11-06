//! Strings that cells refer to by a 16-bit id: hyperlink targets and
//! grapheme cluster tails.

use std::collections::HashMap;

/// A table of distinct strings. Id 0 means "none"; a string's id is its
/// index + 1, so a table holds at most `u16::MAX` strings.
#[derive(Debug, Default)]
pub(super) struct StringTable {
    items: Vec<String>,
    ids: HashMap<String, u16>,
}

impl StringTable {
    const CAPACITY: usize = u16::MAX as usize;

    pub(super) fn get(&self, id: u16) -> Option<&str> {
        let index = usize::from(id).checked_sub(1)?;
        self.items.get(index).map(String::as_str)
    }

    /// The id of `s`, if the table has it.
    pub(super) fn find(&self, s: &str) -> Option<u16> {
        self.ids.get(s).copied()
    }

    /// The id of `s`, adding it if new; `None` when the table is full.
    pub(super) fn insert(&mut self, s: &str) -> Option<u16> {
        if let Some(id) = self.find(s) {
            return Some(id);
        }
        if self.items.len() >= Self::CAPACITY {
            return None;
        }
        self.items.push(s.to_string());
        let id = self.items.len() as u16;
        self.ids.insert(s.to_string(), id);
        Some(id)
    }

    pub(super) fn is_full(&self) -> bool {
        self.items.len() >= Self::CAPACITY
    }

    /// Keeps only the strings whose id is marked in `used` (indexed by id),
    /// and returns the new id of every old one (0 for dropped ones).
    pub(super) fn retain(&mut self, used: &[bool]) -> Vec<u16> {
        let mut remap = vec![0; self.items.len() + 1];
        let old = std::mem::take(&mut self.items);
        self.ids.clear();
        for (index, s) in old.into_iter().enumerate() {
            if used.get(index + 1).copied().unwrap_or(false) {
                self.items.push(s.clone());
                let id = self.items.len() as u16;
                self.ids.insert(s, id);
                remap[index + 1] = id;
            }
        }
        remap
    }

    /// One flag per id (and one for id 0), for marking ids in use.
    pub(super) fn unused(&self) -> Vec<bool> {
        vec![false; self.items.len() + 1]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_strings_share_an_id() {
        let mut table = StringTable::default();
        let a = table.insert("a").unwrap();
        assert_eq!(table.insert("b"), Some(a + 1));
        assert_eq!(table.insert("a"), Some(a));
        assert_eq!(table.get(a), Some("a"));
        assert_eq!(table.get(0), None);
    }

    #[test]
    fn retain_drops_unused_strings_and_renumbers() {
        let mut table = StringTable::default();
        for s in ["a", "b", "c"] {
            table.insert(s);
        }
        let mut used = table.unused();
        used[3] = true;
        let remap = table.retain(&used);
        assert_eq!(remap, vec![0, 0, 0, 1]);
        assert_eq!(table.get(1), Some("c"));
        assert_eq!(table.find("a"), None);
        assert_eq!(table.insert("a"), Some(2));
    }
}
