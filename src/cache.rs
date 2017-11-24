#![allow(dead_code)]
///! Implementation of cache functionality for common use of Writium APIs.
use std::sync::{Arc, Mutex, RwLock};
use writium_framework::prelude::*;
use writium_framework::futures::{Async, Poll};
use writium_framework::futures::future::poll_fn;

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
    pub fn get(&self, id: &str) -> WritiumResult<Arc<RwLock<Src::Value>>> {
        let inner = self.inner.clone();
        let id = id.to_owned();
        poll_fn(move || -> Poll<Arc<RwLock<Src::Value>>, WritiumError> {
            let mut lock = inner.cache.lock().unwrap();
            let pos = lock.iter().position(|&(ref nid, _)| nid == &id);
            if let Some(pos) = pos {
                // Cache found.
                let res = lock.remove(pos);
                lock.push(res.clone());
                return Ok(Async::Ready(res.1))
            } else {
                // Requested resource is not yet cached. Generate now.
                if let Some(new_val) = inner.src.generate(&id) {
                    let new_arc = Arc::new(RwLock::new(new_val));
                    if lock.len() == lock.capacity() {
                        // When this write lock is successfully retrieved, there
                        // should be no other thread accessing it:
                        //
                        // 1. No read guard stays alive;
                        // 2. It's nolonger accessed through `self.inner.cache`.
                        let (old_id, old_val) = lock.remove(0);
                        let mut old_val = if let Ok(rw) = Arc::try_unwrap(old_val) {
                            rw.into_inner().unwrap()
                        } else {
                            warn!("{}: {}", UNEXPECTED_USE_OF_CACHE, id);
                            return Err(WritiumError::internal(UNEXPECTED_USE_OF_CACHE))
                        };
                        inner.src.dispose(&old_id, &mut old_val);
                    }
                    lock.push((id.to_string(), new_arc.clone()));
                    Ok(Async::Ready(new_arc))
                } else {
                    warn!("{}: {}", UNABLE_TO_GEN_CACHE, id);
                    Err(WritiumError::not_found(UNABLE_TO_GEN_CACHE))
                }
            }
        }).into_result()
    }

    /// Remove the object identified by given ID. If the object is not cached,
    /// try recovering its cache from provided source and then remove it. In
    /// case cache generation is needed, the cache stays intact with no cached
    /// object disposed, because the use of generated object is transient.
    pub fn remove(&self, id: &str) -> WritiumResult<Src::Value> {
        let inner = self.inner.clone();
        let id = id.to_owned();
        poll_fn(move || {
            let mut lock = inner.cache.lock().unwrap();
            let pos = lock.iter().position(|&(ref nid, _)| nid == &id);
            if let Some(pos) = pos {
                // Cache found.
                let (old_id, old_val) = lock.remove(pos);
                let mut old_val = if let Ok(rw) = Arc::try_unwrap(old_val) {
                    rw.into_inner().unwrap()
                } else {
                    warn!("{}: {}", UNEXPECTED_USE_OF_CACHE, id);
                    return Err(WritiumError::internal(UNEXPECTED_USE_OF_CACHE))
                };
                inner.src.remove(&old_id, &mut old_val);
                return Ok(Async::Ready(old_val))
            } else {
                // Requested resource is not yet cached. See if we can restore
                // it from source.
                if let Some(mut new_val) = inner.src.generate(&id) {
                    inner.src.remove(&id, &mut new_val);
                    Ok(Async::Ready(new_val))
                } else {
                    warn!("{}: {}", UNABLE_TO_REM_CACHE, id);
                    Err(WritiumError::not_found(UNABLE_TO_REM_CACHE))
                }
            }
        }).into_result()
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
    use ::writium_framework::futures::Future;
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
