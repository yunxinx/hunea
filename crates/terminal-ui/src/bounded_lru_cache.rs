use std::{
    collections::{HashMap, VecDeque},
    hash::Hash,
};

#[derive(Debug)]
pub(crate) struct BoundedLruCache<K, V> {
    values: HashMap<K, V>,
    recent: VecDeque<K>,
    capacity: usize,
}

impl<K, V> BoundedLruCache<K, V>
where
    K: Clone + Eq + Hash,
{
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            values: HashMap::new(),
            recent: VecDeque::new(),
            capacity,
        }
    }

    pub(crate) fn get_cloned(&mut self, key: &K) -> Option<V>
    where
        V: Clone,
    {
        let value = self.values.get(key).cloned()?;
        self.touch(key.clone());
        Some(value)
    }

    pub(crate) fn insert(&mut self, key: K, value: V) {
        self.values.insert(key.clone(), value);
        self.touch(key);
        while self.values.len() > self.capacity {
            let Some(oldest) = self.recent.pop_front() else {
                break;
            };
            self.values.remove(&oldest);
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.values.len()
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub(crate) fn values(&self) -> impl Iterator<Item = &V> {
        self.values.values()
    }

    pub(crate) fn storage_capacity(&self) -> usize {
        self.values.capacity()
    }

    #[cfg(test)]
    pub(crate) fn peek_cloned(&self, key: &K) -> Option<V>
    where
        V: Clone,
    {
        self.values.get(key).cloned()
    }

    fn touch(&mut self, key: K) {
        if let Some(position) = self.recent.iter().position(|candidate| candidate == &key) {
            self.recent.remove(position);
        }
        self.recent.push_back(key);
    }
}

#[cfg(test)]
mod tests {
    use super::BoundedLruCache;

    #[test]
    fn cache_refreshes_recency_and_evicts_the_oldest_key() {
        let mut cache = BoundedLruCache::new(2);
        cache.insert("a", 1);
        cache.insert("b", 2);
        assert_eq!(cache.get_cloned(&"a"), Some(1));

        cache.insert("c", 3);

        assert_eq!(cache.get_cloned(&"a"), Some(1));
        assert_eq!(cache.get_cloned(&"b"), None);
        assert_eq!(cache.get_cloned(&"c"), Some(3));
    }

    #[test]
    fn updating_a_key_does_not_duplicate_its_recency_entry() {
        let mut cache = BoundedLruCache::new(2);
        cache.insert("a", 1);
        cache.insert("a", 2);
        cache.insert("b", 3);
        cache.insert("c", 4);

        assert_eq!(cache.len(), 2);
        assert_eq!(cache.get_cloned(&"a"), None);
        assert_eq!(cache.get_cloned(&"b"), Some(3));
        assert_eq!(cache.get_cloned(&"c"), Some(4));
    }
}
