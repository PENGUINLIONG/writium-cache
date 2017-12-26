#[macro_use]
extern crate log;
extern crate writium_framework;

mod cache;
mod dumb;

pub use cache::{Cache, CacheSource};
pub use dumb::DumbCacheSource;
