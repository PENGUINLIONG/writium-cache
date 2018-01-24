use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::sync::atomic::{AtomicBool, Ordering};
use writium::prelude::*;

const ERR_POISONED_THREAD: &str = "Current thread is poisoned.";

/// Unit of storage in `Cache`.
pub struct CacheItem<T: 'static> {
    id: String,
    data: RwLock<T>,
    is_dirty: AtomicBool,
}
impl<T: 'static> CacheItem<T> {
    pub fn new(id: &str, data: T) -> CacheItem<T> {
        CacheItem {
            id: id.to_owned(),
            data: RwLock::new(data),
            is_dirty: AtomicBool::new(false),
        }
    }

    /// Get the ID of current 
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Lock the internal mutex and return a read guard holding the cached data.
    pub fn read<'guard, 'lock: 'guard>(&'lock self) -> Result<RwLockReadGuard<'guard, T>> {
        self.data.read()
            .map_err(|_| Error::internal(ERR_POISONED_THREAD))
    }
    /// Lock the internal mutex and return a write guard holding the cached
    /// data.
    pub fn write<'guard, 'lock: 'guard>(&'lock self) -> Result<RwLockWriteGuard<'guard, T>> {
        self.set_dirty();
        self.data.write()
            .map_err(|_| Error::internal(ERR_POISONED_THREAD))
    }

    /// If the flag is set, it means that cached data has been modified or is
    /// newly created. The change should be propagated to local storage.
    pub fn is_dirty(&self) -> bool {
        self.is_dirty.load(Ordering::Acquire)
    }
    /// Set the dirty flag on.
    pub(crate) fn set_dirty(&self) {
        self.is_dirty.store(true, Ordering::Release)
    }
}
