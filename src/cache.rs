#![allow(dead_code)]
///! Implementation of cache functionality for common use of Writium APIs.
///!
///! To obey the rule, 'Do not communicate by sharing memory; instead, share
///! memory by communicating,' Writium cache doesn't allow memory borrowing.
use std::sync::RwLock;
use lru_cache::LruCache;

/// Cache for each Writium Api. Any Writium Api can be composited with this
/// struct for cache.
pub struct Cache<Src: 'static + CacheSource> {
    cache: RwLock<LruCache<String, Src::Value>>,
    src: Src,
}
impl<Src: 'static + CacheSource> Cache<Src> {
    pub fn new(capacity: usize, src: Src) -> Cache<Src> {
        Cache {
            cache: RwLock::new(LruCache::new(capacity)),
            src: src,
        }
    }

    // Try to get the object identified by given ID. `None` is returned directly
    // if the value is not cached.
    fn fetch(&self, id: &str) -> Option<Src::Value> {
        let guard = self.cache.read().unwrap();
        if let Some((_, val)) = guard.iter().find(|x| x.0 == id) {
            Some(val.clone())
        } else { None }
    }

    /// Get the object identified by given ID. If the object is not cached, try
    /// generating its cache with provided generation function. If there is no
    /// space for another object, the last recently accessed cache will be
    /// disposed.
    pub fn get(&self, id: &str) -> Option<Src::Value> {
        let cached = self.fetch(id);
        if cached.is_some() { return cached }
        if let Some(new) = self.src.generate(id) {
            let mut guard = self.cache.write().unwrap();
            guard.insert(id.to_string(), new);
        } else { return None }
        self.fetch(id)
    }

    /// Set the object identified by given ID with provided new one. If the
    /// object is not cached, try generating its cache with provided generation
    /// function. If there is no space for another object, the last recently
    /// accessed cache will be disposed and returned.
    pub fn insert(&self, id: &str, new_val: Src::Value) -> Option<Src::Value> {
        let mut guard = self.cache.write().unwrap();
        if let Some(mut old_val) = guard.insert(id.to_string(), new_val) {
            self.src.dispose(id, &mut old_val);
            Some(old_val)
        } else { None }
    }

    /// Remove the object identified by given ID. If the object is not cached,
    /// try generating its cache with provided generation function and then
    /// remove it. In case cache generation is needed. If there is no space for
    /// another object, the last recently accessed cache will be disposed. The
    /// value removed as cache is returned.
    pub fn remove(&self, id: &str) -> Option<Src::Value> {
        let mut guard = self.cache.write().unwrap();
        if let Some(mut old_val) = guard.remove(id) {
            self.src.remove(id, &mut old_val);
            Some(old_val)
        } else { None }
    }

    /// The maximum number of items can be cached at a same time.
    pub fn capacity(&self) -> usize {
        // Only if the thread is poisoned `cache` will be unavailable.
        self.cache.read().unwrap().capacity()
    }

    /// Get the number of items cached.
    pub fn len(&self) -> usize {
        // Only if the thread is poisoned `cache` will be unavailable.
        self.cache.read().unwrap().len()
    }
}

pub trait CacheSource: 'static + Send + Sync {
    type Value: Clone;
    /// Generate a new cached object. Return a value defined by default
    /// configurations. In the course of generation, no state should be created
    /// or written out of RAM, e.g., writing to files or calling to remote
    /// machines.
    fn generate(&self, id: &str) -> Option<Self::Value>;
    /// Dispose a cached object. Implementations should write the value into a
    /// recoverable form of storage, e.g., serializing data into JSON, if
    /// necessary. Cache disposal is an optional process.
    fn dispose(&self, _id: &str, _obj: &mut Self::Value) {}
    /// Remove a cached object. Implementations should remove any associated
    /// data from storage and invalidate any recoverability. Cache removal is an
    /// optional process.
    fn remove(&self, _id: &str, _obj: &mut Self::Value) {}
}

#[cfg(test)]
mod tests {
    // `bool` controls always fail.
    struct TestSource(bool);
    impl super::CacheSource for TestSource {
        type Value = &'static str;
        fn generate(&self, id: &str) -> Option<Self::Value> {
            if self.0 { None }
            else { Some(&["cache0", "cache1", "cache2", "cache3"][id.parse::<usize>().unwrap()]) }
        }
        fn dispose(&self, _id: &str, obj: &mut Self::Value) {
            *obj = "disposed"
        }
        fn remove(&self, _id: &str, obj: &mut Self::Value) {
            *obj = "removed"
        }
    }
    type TestCache = super::Cache<TestSource>;

    fn make_cache(fail: bool) -> TestCache {
        TestCache::new(3, TestSource(fail))
    }

    #[test]
    fn test_cache() {
        let cache = make_cache(false);
        assert_eq!(cache.get("0"), Some("cache0"));
        assert_eq!(cache.get("1"), Some("cache1"));
        assert_eq!(cache.get("2"), Some("cache2"));
    }
    #[test]
    fn test_cache_failure() {
        let cache = make_cache(true);
        assert_eq!(cache.get("0"), None);
        assert_eq!(cache.get("1"), None);
        assert_eq!(cache.get("2"), None);
    }
    #[test]
    fn test_max_cache() {
        let cache = make_cache(false);
        assert_eq!(cache.len(), 0);
        assert_eq!(cache.get("0"), Some("cache0"));
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.get("1"), Some("cache1"));
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.get("2"), Some("cache2"));
        assert_eq!(cache.len(), 3);
        assert_eq!(cache.get("3"), Some("cache3"));
        assert_eq!(cache.len(), 3);
    }
    #[test]
    fn test_max_cache_failure() {
        let cache = make_cache(true);
        assert_eq!(cache.len(), 0);
        assert_eq!(cache.get("0"), None);
        assert_eq!(cache.len(), 0);
        cache.get("1");
        assert_eq!(cache.len(), 0);
        cache.get("2");
        assert_eq!(cache.len(), 0);
    }
    #[test]
    fn test_remove() {
        let cache = make_cache(false);
        cache.get("0");
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.remove("0"), Some("removed"));
        assert_eq!(cache.len(), 0);
    }
}
