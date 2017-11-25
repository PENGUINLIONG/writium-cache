#![allow(dead_code)]
///! Implementation of cache functionality for common use of Writium APIs.
use std::sync::{Arc, Mutex, RwLock};
use writium_framework::prelude::*;
use writium_framework::futures::Future;

const UNABLE_TO_GEN_CACHE: &str = "Unable to generate cache because the resource doesn't exist.";
const UNABLE_TO_REM_CACHE: &str = "Unable to remove cache because the resource doesn't exist.";
const UNEXPECTED_USE_OF_CACHE: &str = "Unexpected use of cache.";

/// Cache for each Writium Api. Any Writium Api can be composited with this
/// struct for cache.
struct CacheInner<Src: 'static + CacheSource> {
    capacity: usize,
    cache: Mutex<Vec<(String, Arc<RwLock<Src::Value>>)>>,
    src: Src,
}
impl<Src: 'static + CacheSource> CacheInner<Src> {
    pub fn new(capacity: usize, src: Src) -> CacheInner<Src> {
        CacheInner {
            capacity: capacity,
            cache: Mutex::new(Vec::with_capacity(capacity)),
            src: src,
        }
    }
}
#[derive(Clone)]
pub struct Cache<Src: 'static + CacheSource> {
    inner: Arc<CacheInner<Src>>
}
impl<Src: 'static + CacheSource> Cache<Src> {
    pub fn new(capacity: usize, src: Src) -> Cache<Src> {
        Cache {
            inner: Arc::new(CacheInner::new(capacity, src)),
        }
    }

    /// Get the object identified by given ID. If the object is not cached, try
    /// recovering its cache from provided source. If there is no space for
    /// another object, the last recently accessed cache will be disposed.
    pub fn create(&self, id: &str) -> WritiumResult<Arc<RwLock<Src::Value>>> {
        self._get(id, true)
    }
    /// Get the object identified by given ID. If the object is not cached,
    /// error will be returned. If there is no space for another object, the
    /// last recently accessed cache will be disposed.
    pub fn get(&self, id: &str) -> WritiumResult<Arc<RwLock<Src::Value>>> {
        self._get(id, false)
    }
    fn _get(&self, id: &str, create: bool) -> WritiumResult<Arc<RwLock<Src::Value>>> {
        let id = id.to_owned();
        let pos = {
            let inner = self.inner.clone();
            let lock = inner.cache.lock().unwrap();
            lock.iter().position(|&(ref nid, _)| nid == &id)
        };
        if let Some(pos) = pos {
            // Cache found.
            let inner = self.inner.clone();
            let mut lock = inner.cache.lock().unwrap();
            let res = lock.remove(pos);
            lock.push(res.clone());
            ok(res.1)
        } else {
            // Requested resource is not yet cached. Load now.
            let inner = self.inner.clone();
            self.inner.clone().src.load(&id, create)
                .map(|new_val| { Arc::new(RwLock::new(new_val)) })
                .join({
                    let mut lock = inner.cache.lock().unwrap();
                    if lock.len() == lock.capacity() {
                        // When this write lock is successfully retrieved, there
                        // should be no other thread accessing it:
                        //
                        // 1. No read guard stays alive;
                        // 2. It's nolonger accessed through `self.inner.cache`.
                        let (old_id, old_val) = lock.remove(0);
                        let mut old_val =
                            if let Ok(rw) = Arc::try_unwrap(old_val) {
                            rw.into_inner().unwrap()
                        } else {
                            warn!("{}: {}", UNEXPECTED_USE_OF_CACHE, id);
                            return err(WritiumError::internal(UNEXPECTED_USE_OF_CACHE))
                        };
                        inner.src.unload(&old_id, &mut old_val)
                    } else {
                        ok(())
                    }
                })
                .map(move |(new_arc, _)| {
                    let mut lock = inner.cache.lock().unwrap();
                    lock.push((id.to_string(), new_arc.clone()));
                    new_arc
                })
                .into_result()
        }
    }

    /// Remove the object identified by given ID. If the object is not cached,
    /// try recovering its cache from provided source and then remove it. In
    /// case cache generation is needed, the cache stays intact with no cached
    /// object disposed, because the use of generated object is transient.
    pub fn remove(&self, id: &str) -> WritiumResult<()> {
        let inner = self.inner.clone();
        let id = id.to_owned();

        let mut lock = inner.cache.lock().unwrap();
        let pos = lock.iter().position(|&(ref nid, _)| nid == &id);
        if let Some(pos) = pos {
            // Cache found.
            let (old_id, _) = lock.remove(pos);
            inner.src.remove(&old_id).into_result()
        } else {
            // Requested resource is not yet cached. See if we can restore
            // it from source.
            let f_load = inner.src.load(&id, false);
            let f_rm = inner.src.remove(&id);
            f_load.and_then(|_| f_rm).into_result()
        }
    }

    /// The maximum number of items can be cached at a same time.
    pub fn capacity(&self) -> usize {
        // Only if the thread is poisoned `cache` will be unavailable.
        self.inner.cache.lock().unwrap().capacity()
    }

    /// Get the number of items cached.
    pub fn len(&self) -> usize {
        // Only if the thread is poisoned `cache` will be unavailable.
        self.inner.cache.lock().unwrap().len()
    }
}

pub trait CacheSource: 'static + Send + Sync {
    type Value: Clone;
    /// Create a new cached object. Return a value defined by default
    /// configurations if `create` is set. In the course of creation, no state
    /// should be stored out of RAM, e.g., writing to files or calling to remote
    /// machines. The future returned MUST NOT excecute before the call to
    /// `poll()`.
    fn load(&self, id: &str, create: bool) -> WritiumResult<Self::Value>;
    /// Unload a cached object. Implementations should write the value into a
    /// recoverable form of storage, e.g., serializing data into JSON, if
    /// necessary. Cache unloading is an optional process. The future returned
    /// MUST NOT excecute before the call to `poll()`.
    fn unload(&self, _id: &str, _obj: &Self::Value) -> WritiumResult<()> { ok(()) }
    /// Remove a cached object. Implementations should remove any associated
    /// data from storage and invalidate any recoverability. Cache removal is an
    /// optional process. The future returned MUST NOT excecute before the call
    /// to `poll()`.
    fn remove(&self, _id: &str) -> WritiumResult<()> { ok(()) }
}
impl<Src: 'static + CacheSource> Drop for Cache<Src> {
    /// Implement drop so that modified cached data can be returned to source
    /// properly.
    fn drop(&mut self) {
        info!("Writing cached data back to source...");
        let mut lock = self.inner.cache.lock().unwrap();
        while let Some((id, val)) = lock.pop() {
            if let Ok(val) = Arc::try_unwrap(val) {
                self.inner.src.unload(&id, &mut val.into_inner().unwrap());
            } else {
                warn!("{}: {}", UNEXPECTED_USE_OF_CACHE, id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use ::writium_framework::futures::Future;
    // `bool` controls always fail.
    struct TestSource(bool);
    impl super::CacheSource for TestSource {
        type Value = &'static str;
        fn load(&self, id: &str) -> Option<Self::Value> {
            if self.0 { None }
            else { Some(&["cache0", "cache1", "cache2", "cache3"][id.parse::<usize>().unwrap()]) }
        }
        fn unload(&self, _id: &str, obj: &mut Self::Value) {
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
        assert!(cache.get("0").wait().is_ok());
        assert!(cache.get("1").wait().is_ok());
        assert!(cache.get("2").wait().is_ok());
    }
    #[test]
    fn test_cache_failure() {
        let cache = make_cache(true);
        assert!(cache.get("0").wait().is_err());
        assert!(cache.get("1").wait().is_err());
        assert!(cache.get("2").wait().is_err());
    }
    #[test]
    fn test_max_cache() {
        let cache = make_cache(false);
        assert!(cache.len() == 0);
        assert!(cache.get("0").wait().is_ok());
        assert!(cache.len() == 1);
        assert!(cache.get("1").wait().is_ok());
        assert!(cache.len() == 2);
        assert!(cache.get("2").wait().is_ok());
        assert!(cache.len() == 3);
        assert!(cache.get("3").wait().is_ok());
        assert!(cache.len() == 3);
    }
    #[test]
    fn test_max_cache_failure() {
        let cache = make_cache(true);
        assert!(cache.len() == 0);
        assert!(cache.get("0").wait().is_err());
        assert!(cache.len() == 0);
        cache.get("1");
        assert!(cache.len() == 0);
        cache.get("2");
        assert!(cache.len() == 0);
    }
    #[test]
    fn test_remove() {
        let cache = make_cache(false);
        assert!(cache.get("0").wait().is_ok());
        assert!(cache.len() == 1);
        assert!(cache.remove("0").wait().is_ok());
        assert!(cache.len() == 0);
        assert!(cache.remove("0").wait().is_ok());
    }
}
