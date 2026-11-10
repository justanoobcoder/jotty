//! Values that cells refer to by a 16-bit id: hyperlink targets, grapheme
//! cluster tails, and the combinations of those and underline colors that
//! a cell carries (`Extra`).

use std::borrow::Borrow;
use std::collections::HashMap;
use std::hash::Hash;

/// A table of distinct values. Id 0 means "none"; a value's id is its
/// index + 1, so a table holds at most `u16::MAX` values.
#[derive(Debug)]
pub(super) struct Table<T> {
    items: Vec<T>,
    ids: HashMap<T, u16>,
    /// At most `u16::MAX`; tests lower it to fill tables quickly.
    capacity: usize,
}

pub(super) type StringTable = Table<String>;

impl<T> Default for Table<T> {
    fn default() -> Self {
        Table {
            items: Vec::new(),
            ids: HashMap::new(),
            capacity: u16::MAX as usize,
        }
    }
}

impl<T: Clone + Eq + Hash> Table<T> {
    #[cfg(test)]
    pub(super) fn set_capacity(&mut self, capacity: usize) {
        self.capacity = capacity.min(u16::MAX as usize);
    }

    /// The value with `id`; `None` for 0 or an id the table doesn't have.
    pub(super) fn get(&self, id: u16) -> Option<&T> {
        let index = usize::from(id).checked_sub(1)?;
        self.items.get(index)
    }

    /// The id of `value`, if the table has it.
    pub(super) fn find<Q>(&self, value: &Q) -> Option<u16>
    where
        T: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.ids.get(value).copied()
    }

    /// The id of `value`, adding it if new; `None` when the table is full.
    pub(super) fn insert<Q>(&mut self, value: &Q) -> Option<u16>
    where
        T: Borrow<Q>,
        Q: Hash + Eq + ToOwned<Owned = T> + ?Sized,
    {
        if let Some(id) = self.find(value) {
            return Some(id);
        }
        if self.is_full() {
            return None;
        }
        self.items.push(value.to_owned());
        let id = self.items.len() as u16;
        self.ids.insert(value.to_owned(), id);
        Some(id)
    }

    pub(super) fn is_full(&self) -> bool {
        self.items.len() >= self.capacity
    }

    /// Keeps only the values whose id is marked in `used` (indexed by id),
    /// and returns the new id of every old one (0 for dropped ones).
    pub(super) fn retain(&mut self, used: &[bool]) -> Vec<u16> {
        self.retain_map(used, T::clone)
    }

    /// Like `retain`, replacing each kept value with `map` of it. Values
    /// that `map` makes equal must not both be kept.
    pub(super) fn retain_map(&mut self, used: &[bool], mut map: impl FnMut(&T) -> T) -> Vec<u16> {
        let mut remap = vec![0; self.items.len() + 1];
        let old = std::mem::take(&mut self.items);
        self.ids.clear();
        for (index, value) in old.iter().enumerate() {
            if used.get(index + 1).copied().unwrap_or(false) {
                let value = map(value);
                self.items.push(value.clone());
                let id = self.items.len() as u16;
                self.ids.insert(value, id);
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
        assert_eq!(table.get(a).map(String::as_str), Some("a"));
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
        assert_eq!(table.get(1).map(String::as_str), Some("c"));
        assert_eq!(table.find("a"), None);
        assert_eq!(table.insert("a"), Some(2));
    }

    #[test]
    fn retain_map_rewrites_kept_values() {
        let mut table = Table::<(u16, u16)>::default();
        for value in [(1, 1), (2, 2), (3, 3)] {
            table.insert(&value);
        }
        let mut used = table.unused();
        used[1] = true;
        used[3] = true;
        let remap = table.retain_map(&used, |&(a, b)| (a * 10, b));
        assert_eq!(remap, vec![0, 1, 0, 2]);
        assert_eq!(table.get(2), Some(&(30, 3)));
        assert_eq!(table.find(&(30, 3)), Some(2));
        assert_eq!(table.find(&(3, 3)), None);
    }
}
