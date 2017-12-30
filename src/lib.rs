#[macro_use]
extern crate log;
extern crate writium;

mod cache;
mod dumb;

pub use cache::{Cache, CacheSource};
pub use dumb::DumbCacheSource;
