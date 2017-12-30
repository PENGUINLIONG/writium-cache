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
use std::sync::{Arc, Mutex, RwLock};
use writium::prelude::*;

const ERR_UNEXPECTED_OCCUPANCY: &str = "Unexpected use of cache.";
const ERR_POISONED_THREAD: &str = "Current thread is poisoned.";

/// Cache for each Writium Api. Any Writium Api can be composited with this
/// struct for cache.
#[derive(Clone)]
pub struct Cache<T: 'static> {
    cache: Arc<Mutex<Vec<(String, Arc<RwLock<T>>)>>>,
    src: Arc<CacheSource<Value=T>>,
}
impl<T: 'static> Cache<T> {
    pub fn new<Src>(capacity: usize, src: Src) -> Cache<T>
        where Src: 'static + CacheSource<Value=T> {
        Cache {
            cache: Arc::new(Mutex::new(Vec::with_capacity(capacity))),
            src: Arc::new(src),
        }
    }

    /// Get the object identified by given ID. If the object is not cached, try
    /// recovering its cache from provided source. If there is no space for
    /// another object, the last recently accessed cache will be disposed.
    pub fn create(&self, id: &str) -> Result<Arc<RwLock<T>>> {
        self._get(&id, true)
    }
    /// Get the object identified by given ID. If the object is not cached,
    /// error will be returned. If there is no space for another object, the
    /// last recently accessed cache will be disposed.
    pub fn get(&self, id: &str) -> Result<Arc<RwLock<T>>> {
        self._get(&id, false)
    }
    fn _get(&self, id: &str, create: bool) -> Result<Arc<RwLock<T>>> {
        let mut cache = if let Ok(locked) = self.cache.lock() {
            locked
        } else {
            return Err(Error::internal(ERR_POISONED_THREAD))
        };
        
        if let Some(pos) = cache.iter().position(|&(ref jd, _)| jd == id) {
            // Cache found.
            let rsc = cache.remove(pos);
            let rv = rsc.1.clone();
            cache.push(rsc);
            Ok(rv)
        } else {
            // Requested resource is not yet cached. Load now.
            if cache.len() == cache.capacity() {
                // When this write lock is successfully retrieved, there should
                // be no other thread accessing it:
                //
                // 1. No read guard stays alive;
                // 2. It's no longer accessed through `self.0.cache`.
                Arc::try_unwrap(cache.remove(0).1)
                    .map_err(|_| Error::internal(ERR_UNEXPECTED_OCCUPANCY))?
                    .into_inner()
                    .map_err(|_| Error::internal(ERR_POISONED_THREAD))
                    .and_then(|mut val| self.src.unload(id, &mut val))?;
            }
            let arc = Arc::new(RwLock::new(self.src.load(&id, create)?));
            cache.push((id.to_string(), arc.clone()));
            Ok(arc)
        }
    }

    /// Remove the object identified by given ID.
    pub fn remove(&self, id: &str) -> Result<()> {
        let mut cache = self.cache.lock().unwrap();
        cache.iter()
            .position(|&(ref nid, _)| nid == &id)
            .map(|pos| cache.remove(pos));
        self.src.remove(&id)
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
        info!("Writing cached data back to source...");
        let mut lock = self.cache.lock().unwrap();
        while let Some((id, val)) = lock.pop() {
            if let Ok(val) = Arc::try_unwrap(val) {
                let unload = self.src.unload(&id, &mut val.into_inner().unwrap());
                if let Err(err) = unload {
                    warn!("Unable to unload '{}': {}", id, err);
                }
            } else {
                warn!("Unexpected use of '{}'", id);
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
