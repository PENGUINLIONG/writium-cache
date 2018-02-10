///! Implementation of caching for common use of Writium APIs.
///!
///! # Why I should use caches?
///!
///! The use of cache system can help you reduce disk I/O so that more fluent
///! execution can be achieved.
///!
///! There is a question you may ask: why not simply use external caching
///! systems (like Nginx)? The answer is, `writium-cache` and Nginx are totally
///! different. Nginx caches *already generated* contents but `writium-cache`
///! *prevents I/O blocking* of working threads. It's needed because Writium API
///! calls are *synchronized* for you to write clean codes, before the stable.
///!
///! # About pending
///!
///! For now, `writium-cache` doesn't offer any multi-threading features, but it
///! guarantees that everything won't go wrong in such context. There is only
///! one thread is involved at a time for a single `Cache` object. Everytime an
///! uncached item is requested, an cached item needs to be unloaded/removed,
///! heavy I/O might come up. In such period of time I/O-ing, the item is
///! attributed 'pending'. Items in such state is not exposed to users. And it
///! can influence the entire system's efficiency seriously by blocking threads.
///! Such outcome is undesirable commonly. Thus, 'pending' state is considered a
///! performance issue and should be fixed in future versions.
///!
///! There are two cases an item is in 'pending' state:
///!
///! 1. Cached data was dirty and is now 'abandoned' by `Cache` - a
///! corresponding `CacheSource` has been trying Writing the data back to
///! local storage. If the data is requested again after this intermediate
///! state, the state will be restored to `Intact`. When unloading is
///! finished, data is written back to storage and is removed from the owning
///! `Cache`.
///!
///! 2. Cached data is being removed by a corresponding `CacheSource`. If the
///! data is requested again after this intermediate state, the state will be
///! restored to `Dirty` (as a new instance is created). When removal is
///! finished, data is removed from storage (as well as the owning `Cache`, if
///! the data was loaded).
use std::sync::{Arc, Mutex};
use writium::prelude::*;
use item::CacheItem;

const ERR_POISONED_THREAD: &str = "Current thread is poisoned.";

/// Cache for each Writium Api. Any Writium Api can be composited with this
/// struct for cache.
pub struct Cache<T: 'static> {
    /// In `cache`, the flag `is_dirty` of an item indicates whether it should
    /// be written back to source.
    cache: Mutex<Vec<Arc<CacheItem<T>>>>,
    src: Box<CacheSource<Value=T>>,
}
impl<T: 'static> Cache<T> {
    pub fn new<Src>(capacity: usize, src: Src) -> Cache<T>
        where Src: 'static + CacheSource<Value=T> {
        Cache {
            cache: Mutex::new(Vec::with_capacity(capacity)),
            src: Box::new(src),
        }
    }
    /// Get the object identified by given ID. If the object is not cached, try
    /// recovering its cache from provided source. If there is no space for
    /// another object, the last recently accessed cache will be disposed.
    pub fn create(&self, id: &str) -> Result<Arc<CacheItem<T>>> {
        self._get(&id, true)
    }
    /// Get the object identified by given ID. If the object is not cached,
    /// error will be returned. If there is no space for another object, the
    /// last recently accessed cache will be disposed.
    pub fn get(&self, id: &str) -> Result<Arc<CacheItem<T>>> {
        self._get(&id, false)
    }

    fn _get(&self, id: &str, create: bool) -> Result<Arc<CacheItem<T>>> {
        // Not intended to introduce too much complexity.
        let mut cache = self.cache.lock()
            .map_err(|_| Error::internal(ERR_POISONED_THREAD))?;
        if let Some(pos) = cache.iter()
            .position(|item| item.id() == id) {
            // Cache found.
            let arc = cache.remove(pos);
            cache.insert(0, arc.clone());
            return Ok(arc)
        } else {
            // Requested resource is not yet cached. Load now.
            let new_item = CacheItem::new(id, self.src.load(id, create)?);
            let new_arc = Arc::new(new_item);
            // Not actually caching anything when capacity is 0.
            if cache.capacity() == 0 {
                return Ok(new_arc)
            }
            // Remove the least-recently-used item from collection.
            if cache.len() == cache.capacity() {
                let lru_item = cache.pop().unwrap();
                // Unload items only when they are dirty.
                if lru_item.is_dirty() {
                    let data = lru_item.write()?;
                    if let Err(err) = self.src.unload(lru_item.id(), &*data) {
                        error!("Unable to unload '{}': {}", id, err);
                    }   
                }
            }
            cache.insert(0, new_arc.clone());
            return Ok(new_arc)
        }
    }

    /// Remove the object identified by given ID.
    pub fn remove(&self, id: &str) -> Result<()> {
        let mut cache = self.cache.lock().unwrap();
        cache.iter()
            .position(|nid| nid.id() == id)
            .map(|pos| cache.remove(pos));
        self.src.remove(&id)
    }

    /// The maximum number of items can be cached at a same time. Tests only.
    #[cfg(test)]
    pub fn capacity(&self) -> usize {
        // Only if the thread is poisoned `cache` will be unavailable.
        self.cache.lock().unwrap().capacity()
    }

    /// Get the number of items cached. Tests only.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        // Only if the thread is poisoned `cache` will be unavailable.
        self.cache.lock().unwrap().len()
    }
}

/// A source where cache can be generated from.
pub trait CacheSource: 'static + Send + Sync {
    type Value: 'static;
    /// Create a new cached object. Return a value defined by default
    /// configurations if `create` is set. In the course of creation, no state
    /// should be stored out of RAM, e.g., writing to files or calling to remote
    /// machines.
    fn load(&self, id: &str, create: bool) -> Result<Self::Value>;
    /// Unload a cached object. Implementations should write the value into a
    /// recoverable form of storage, e.g., serializing data into JSON, if
    /// necessary. Cache unloading is an optional process.
    fn unload(&self, _id: &str, _obj: &Self::Value) -> Result<()> {
        Ok(())
    }
    /// Remove a cached object. Implementations should remove any associated
    /// data from storage and invalidate any recoverability. If a resource
    /// doesn't exist, the source shouldn't report an error. Cache removal is an
    /// optional process.
    fn remove(&self, _id: &str) -> Result<()> {
        Ok(())
    }
}
impl<T: 'static> Drop for Cache<T> {
    /// Implement drop so that modified cached data can be returned to source
    /// properly.
    fn drop(&mut self) {
        let mut lock = self.cache.lock().unwrap();
        while let Some(item) = lock.pop() {
            if !item.is_dirty() { continue }
            let guard = item.write().unwrap();
            if let Err(err) = self.src.unload(item.id(), &guard) {
                warn!("Unable to unload '{}': {}", item.id(), err);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use writium::prelude::*;
    // `bool` controls always fail.
    struct TestSource(bool);
    impl super::CacheSource for TestSource {
        type Value = &'static str;
        fn load(&self, id: &str, _create: bool) -> Result<Self::Value> {
            if self.0 { Err(Error::not_found("")) }
            else { Ok(&["cache0", "cache1", "cache2", "cache3"][id.parse::<usize>().unwrap()]) }
        }
        fn unload(&self, _id: &str, _obj: &Self::Value) -> Result<()> {
            Ok(())
        }
        fn remove(&self, _id: &str) -> Result<()> {
            Ok(())
        }
    }
    type TestCache = super::Cache<&'static str>;

    fn make_cache(fail: bool) -> TestCache {
        TestCache::new(3, TestSource(fail))
    }

    #[test]
    fn test_cache() {
        let cache = make_cache(false);
        assert!(cache.get("0").is_ok());
        assert!(cache.get("1").is_ok());
        assert!(cache.get("2").is_ok());
    }
    #[test]
    fn test_cache_failure() {
        let cache = make_cache(true);
        assert!(cache.get("0").is_err());
        assert!(cache.get("1").is_err());
        assert!(cache.get("2").is_err());
    }
    #[test]
    fn test_max_cache() {
        let cache = make_cache(false);
        assert!(cache.len() == 0);
        assert!(cache.get("0").is_ok());
        assert!(cache.len() == 1);
        assert!(cache.get("1").is_ok());
        assert!(cache.len() == 2);
        assert!(cache.get("2").is_ok());
        assert!(cache.len() == 3);
        assert!(cache.get("3").is_ok());
        assert!(cache.len() == 3);
    }
    #[test]
    fn test_max_cache_failure() {
        let cache = make_cache(true);
        assert!(cache.len() == 0);
        assert!(cache.get("0").is_err());
        assert!(cache.len() == 0);
        assert!(cache.get("1").is_err());
        assert!(cache.len() == 0);
        assert!(cache.get("2").is_err());
        assert!(cache.len() == 0);
    }
    #[test]
    fn test_remove() {
        let cache = make_cache(false);
        assert!(cache.get("0").is_ok());
        assert!(cache.len() == 1);
        assert!(cache.remove("0").is_ok());
        assert!(cache.len() == 0);
        assert!(cache.remove("0").is_ok());
    }
}
