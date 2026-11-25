//! [`SideTable`]: extra per-entity data stored next to an [`crate::IdMap`],
//! indexed by slot and checked against the full key, so stale keys never
//! see another entity's data.

use serde::{Deserialize, Serialize};
use slotmap::Key;

use crate::idmap::key_index;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SideTable<K: Key, V> {
    entries: Vec<Option<(K, V)>>,
}

impl<K: Key, V> Default for SideTable<K, V> {
    fn default() -> Self {
        SideTable { entries: Vec::new() }
    }
}

impl<K: Key, V> SideTable<K, V> {
    pub fn get(&self, k: K) -> Option<&V> {
        match self.entries.get(key_index(k)) {
            Some(Some((key, v))) if *key == k => Some(v),
            _ => None,
        }
    }

    pub fn get_mut(&mut self, k: K) -> Option<&mut V> {
        match self.entries.get_mut(key_index(k)) {
            Some(Some((key, v))) if *key == k => Some(v),
            _ => None,
        }
    }

    pub fn insert(&mut self, k: K, v: V) {
        let i = key_index(k);
        if self.entries.len() <= i {
            self.entries.resize_with(i + 1, || None);
        }
        self.entries[i] = Some((k, v));
    }

    pub fn remove(&mut self, k: K) -> Option<V> {
        let slot = self.entries.get_mut(key_index(k))?;
        if matches!(slot, Some((key, _)) if *key == k) {
            slot.take().map(|(_, v)| v)
        } else {
            None
        }
    }

    /// Get, inserting `V::default()` first if missing.
    pub fn entry(&mut self, k: K) -> &mut V
    where
        V: Default,
    {
        if self.get(k).is_none() {
            self.insert(k, V::default());
        }
        self.get_mut(k).expect("just inserted")
    }

    /// All entries, in slot order.
    pub fn iter(&self) -> impl Iterator<Item = (K, &V)> + '_ {
        self.entries.iter().filter_map(|e| e.as_ref().map(|(k, v)| (*k, v)))
    }

    pub fn values(&self) -> impl Iterator<Item = &V> + '_ {
        self.entries.iter().filter_map(|e| e.as_ref().map(|(_, v)| v))
    }

    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut V> + '_ {
        self.entries.iter_mut().filter_map(|e| e.as_mut().map(|(_, v)| v))
    }

    /// Drop entries whose key `alive` rejects.
    pub fn retain_keys(&mut self, mut alive: impl FnMut(K) -> bool) {
        for e in &mut self.entries {
            if matches!(e, Some((k, _)) if !alive(*k)) {
                *e = None;
            }
        }
    }
}
