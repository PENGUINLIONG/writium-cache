use std::marker::PhantomData;
use writium::prelude::*;
use cache::CacheSource;

const ERR_DUMB: &'static str = "Dumb cache is used, nothing is extracted.";

pub struct DumbCacheSource<T: 'static + Send + Sync>(PhantomData<T>);
impl<T: 'static + Send + Sync> DumbCacheSource<T> {
    pub fn new() -> DumbCacheSource<T> {
        DumbCacheSource(PhantomData)
    }
}
impl<T: 'static + Send + Sync> CacheSource for DumbCacheSource<T> {
    type Value = T;
    fn load(&self, _id: &str, _create: bool) -> Result<T> {
        Err(Error::internal(ERR_DUMB))
    }
}
