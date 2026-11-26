//! [`IdMap`]: a generational arena with stable keys, like `slotmap::SlotMap`
//! (same key types, a subset of its API), but whose serialised form includes
//! the free list. So a loaded world hands out exactly the same ids as the
//! original would have — save/load keeps the simulation in lockstep.

use std::fmt;
use std::marker::PhantomData;
use std::ops::{Index, IndexMut};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use slotmap::{Key, KeyData};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
struct Slot<V> {
    /// Odd while occupied, even while vacant.
    version: u32,
    value: Option<V>,
}

#[derive(Clone, PartialEq)]
pub struct IdMap<K: Key, V> {
    slots: Vec<Slot<V>>,
    /// Vacant slot indices; the last one is reused first.
    free: Vec<u32>,
    len: usize,
    _k: PhantomData<fn(K) -> K>,
}

impl<K: Key, V> Default for IdMap<K, V> {
    fn default() -> Self {
        IdMap {
            slots: Vec::new(),
            free: Vec::new(),
            len: 0,
            _k: PhantomData,
        }
    }
}

fn make_key<K: Key>(idx: usize, version: u32) -> K {
    KeyData::from_ffi((u64::from(version) << 32) | idx as u64).into()
}

fn split_key<K: Key>(k: K) -> (usize, u32) {
    let ffi = k.data().as_ffi();
    ((ffi & 0xffff_ffff) as usize, (ffi >> 32) as u32)
}

/// Dense index of the slot behind `k` (stable while the entry lives; reused
/// after removal). Lets side tables be plain vectors.
pub fn key_index<K: Key>(k: K) -> usize {
    split_key(k).0
}

impl<K: Key, V> IdMap<K, V> {
    /// Number of slots (occupied or not); every key's `key_index` is below it.
    pub fn slot_count(&self) -> usize {
        self.slots.len()
    }

    pub fn new() -> Self {
        Self::default()
    }

    /// Same as `new`; mirrors `SlotMap::with_key`.
    pub fn with_key() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn insert(&mut self, value: V) -> K {
        self.insert_with_key(|_| value)
    }

    pub fn insert_with_key(&mut self, f: impl FnOnce(K) -> V) -> K {
        self.len += 1;
        if let Some(idx) = self.free.pop() {
            let slot = &mut self.slots[idx as usize];
            slot.version = slot.version.wrapping_add(1) | 1;
            let key = make_key(idx as usize, slot.version);
            slot.value = Some(f(key));
            key
        } else {
            let idx = self.slots.len();
            assert!(idx < u32::MAX as usize, "IdMap full");
            let key = make_key(idx, 1);
            self.slots.push(Slot {
                version: 1,
                value: Some(f(key)),
            });
            key
        }
    }

    fn slot(&self, k: K) -> Option<&Slot<V>> {
        let (idx, version) = split_key(k);
        self.slots
            .get(idx)
            .filter(|s| s.version == version && s.value.is_some())
    }

    /// Key of the entry in slot `idx`, if that slot is occupied (lets a
    /// caller visit a subset of slots, e.g. every n-th).
    pub fn key_at(&self, idx: usize) -> Option<K> {
        let s = self.slots.get(idx)?;
        s.value.as_ref().map(|_| make_key(idx, s.version))
    }

    pub fn contains_key(&self, k: K) -> bool {
        self.slot(k).is_some()
    }

    pub fn get(&self, k: K) -> Option<&V> {
        self.slot(k).and_then(|s| s.value.as_ref())
    }

    pub fn get_mut(&mut self, k: K) -> Option<&mut V> {
        let (idx, version) = split_key(k);
        self.slots
            .get_mut(idx)
            .filter(|s| s.version == version)
            .and_then(|s| s.value.as_mut())
    }

    pub fn remove(&mut self, k: K) -> Option<V> {
        let (idx, version) = split_key(k);
        let slot = self.slots.get_mut(idx).filter(|s| s.version == version)?;
        let value = slot.value.take()?;
        slot.version = slot.version.wrapping_add(1);
        self.free.push(idx as u32);
        self.len -= 1;
        Some(value)
    }

    /// Keep only the entries for which `f` returns true.
    pub fn retain(&mut self, mut f: impl FnMut(K, &mut V) -> bool) {
        let doomed: Vec<K> = self.iter_mut().filter_map(|(k, v)| (!f(k, v)).then_some(k)).collect();
        for k in doomed {
            self.remove(k);
        }
    }

    pub fn clear(&mut self) {
        let keys: Vec<K> = self.keys().collect();
        for k in keys {
            self.remove(k);
        }
    }

    /// Entries in slot order (deterministic).
    pub fn iter(&self) -> impl Iterator<Item = (K, &V)> + '_ {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.value.as_ref().map(|v| (make_key(i, s.version), v)))
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (K, &mut V)> + '_ {
        self.slots
            .iter_mut()
            .enumerate()
            .filter_map(|(i, s)| s.value.as_mut().map(|v| (make_key(i, s.version), v)))
    }

    pub fn keys(&self) -> impl Iterator<Item = K> + '_ {
        self.iter().map(|(k, _)| k)
    }

    pub fn values(&self) -> impl Iterator<Item = &V> + '_ {
        self.slots.iter().filter_map(|s| s.value.as_ref())
    }

    /// The same map (same keys, same free list) with every value converted.
    pub fn map_values<W>(self, mut f: impl FnMut(V) -> W) -> IdMap<K, W> {
        IdMap {
            slots: self
                .slots
                .into_iter()
                .map(|s| Slot {
                    version: s.version,
                    value: s.value.map(&mut f),
                })
                .collect(),
            free: self.free,
            len: self.len,
            _k: PhantomData,
        }
    }

    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut V> + '_ {
        self.slots.iter_mut().filter_map(|s| s.value.as_mut())
    }
}

impl<K: Key, V> Index<K> for IdMap<K, V> {
    type Output = V;
    fn index(&self, k: K) -> &V {
        self.get(k).expect("invalid IdMap key")
    }
}

impl<K: Key, V> IndexMut<K> for IdMap<K, V> {
    fn index_mut(&mut self, k: K) -> &mut V {
        self.get_mut(k).expect("invalid IdMap key")
    }
}

impl<'a, K: Key, V> IntoIterator for &'a IdMap<K, V> {
    type Item = (K, &'a V);
    type IntoIter = Box<dyn Iterator<Item = (K, &'a V)> + 'a>;
    fn into_iter(self) -> Self::IntoIter {
        Box::new(self.iter())
    }
}

impl<'a, K: Key, V> IntoIterator for &'a mut IdMap<K, V> {
    type Item = (K, &'a mut V);
    type IntoIter = Box<dyn Iterator<Item = (K, &'a mut V)> + 'a>;
    fn into_iter(self) -> Self::IntoIter {
        Box::new(self.iter_mut())
    }
}

impl<K: Key, V: fmt::Debug> fmt::Debug for IdMap<K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

#[derive(Serialize, Deserialize)]
struct Repr<V> {
    slots: Vec<Slot<V>>,
    free: Vec<u32>,
}

impl<K: Key, V: Serialize> Serialize for IdMap<K, V> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct ReprRef<'a, V> {
            slots: &'a [Slot<V>],
            free: &'a [u32],
        }
        ReprRef {
            slots: &self.slots,
            free: &self.free,
        }
        .serialize(s)
    }
}

impl<'de, K: Key, V: Deserialize<'de>> Deserialize<'de> for IdMap<K, V> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let Repr { slots, free } = Repr::<V>::deserialize(d)?;
        let mut is_free = vec![false; slots.len()];
        for &i in &free {
            let i = i as usize;
            if i >= slots.len() || is_free[i] || slots[i].value.is_some() {
                return Err(de::Error::custom("IdMap: bad free list"));
            }
            is_free[i] = true;
        }
        let mut len = 0;
        for (i, s) in slots.iter().enumerate() {
            let occupied = s.value.is_some();
            if occupied != (s.version % 2 == 1) || occupied == is_free[i] {
                return Err(de::Error::custom("IdMap: inconsistent slot"));
            }
            len += usize::from(occupied);
        }
        Ok(IdMap {
            slots,
            free,
            len,
            _k: PhantomData,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    slotmap::new_key_type! { struct TestId; }

    #[test]
    fn insert_remove_reuse_and_stale_keys() {
        let mut m: IdMap<TestId, i32> = IdMap::new();
        let a = m.insert(1);
        let b = m.insert(2);
        assert_eq!(m.len(), 2);
        assert_eq!(m.remove(a), Some(1));
        assert_eq!(m.remove(a), None);
        assert!(!m.contains_key(a));
        let c = m.insert(3);
        assert_ne!(a, c, "reused slot gets a new version");
        assert_eq!(m.get(a), None);
        assert_eq!(m[c], 3);
        assert_eq!(m[b], 2);
        assert_eq!(m.values().copied().collect::<Vec<_>>(), vec![3, 2]);
        assert!(!m.contains_key(TestId::null()));
    }

    #[test]
    fn serde_round_trip_preserves_future_keys() {
        let mut m: IdMap<TestId, u8> = IdMap::new();
        let keys: Vec<TestId> = (0..6).map(|i| m.insert(i)).collect();
        for &k in &[keys[1], keys[4], keys[2]] {
            m.remove(k);
        }
        let bytes = postcard::to_stdvec(&m).unwrap();
        let mut n: IdMap<TestId, u8> = postcard::from_bytes(&bytes).unwrap();
        assert!(m == n);
        for i in 0..4 {
            assert_eq!(m.insert(10 + i), n.insert(10 + i));
        }
    }
}
