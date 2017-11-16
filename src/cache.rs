#![allow(dead_code)]
///! Implementation of cache functionality for common use of Writium APIs.
use std::sync::{Arc, Mutex, RwLock};
use lru_cache::LruCache;

/// Cache for each Writium Api. Any Writium Api can be composited with this
/// struct for cache.
pub struct Cache<Src: 'static + CacheSource> {
    cache: Mutex<LruCache<String, Arc<RwLock<Src::Value>>>>,
    src: Src,
}
impl<Src: 'static + CacheSource> Cache<Src> {
    pub fn new(capacity: usize, src: Src) -> Cache<Src> {
        Cache {
            cache: Mutex::new(LruCache::new(capacity)),
            src: src,
        }
    }

    /// Get the object identified by given ID. If the object is not cached, try
    /// generating its cache with provided generation function. If there is no
    /// space for another object, the last recently accessed cache will be
    /// disposed.
    pub fn get(&self, id: &str) -> Option<Arc<RwLock<Src::Value>>> {
        let mut lock = self.cache.lock().unwrap();
        // Check if the resource is already cached.
        if let Some(cached) = lock.get_mut(id) {
            return Some(cached.clone())
        }
        // Requested resource is not yet cached. Generate now.
        if let Some(new_val) = self.src.generate(id) {
            let new_arc = Arc::new(RwLock::new(new_val));
            lock.insert(id.to_string(), new_arc.clone());
            Some(new_arc)
        } else {
            None
        }
    }

    /// Remove the object identified by given ID. If the object is not cached,
    /// try generating its cache with provided generation function and then
    /// remove it. In case cache generation is needed. If there is no space for
    /// another object, the last recently accessed cache will be disposed. The
    /// value removed as cache is returned.
    pub fn remove(&self, id: &str) -> Option<Src::Value> {
        let mut lock = self.cache.lock().unwrap();
        if let Some(old_val) = lock.remove(id) {
            use std::ops::DerefMut;
            // When this write lock is successfully retrieved, there should be
            // no other thread accessing it:
            //
            // 1. No read guard stays alive;
            // 2. It's nolonger accessed through `self.cache`.
            self.src.remove(id, old_val.write().unwrap().deref_mut());
            if let Ok(rwlock) = Arc::try_unwrap(old_val) {
                Some(rwlock.into_inner().unwrap())
            } else {
                panic!("unexpected use of cache");
            }
        } else { None }
    }

    /// The maximum number of items can be cached at a same time.
    pub fn capacity(&self) -> usize {
        // Only if the thread is poisoned `cache` will be unavailable.
        self.cache.lock().unwrap().capacity()
    }

    /// Get the number of items cached.
    pub fn len(&self) -> usize {
        // Only if the thread is poisoned `cache` will be unavailable.
        self.cache.lock().unwrap().len()
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
