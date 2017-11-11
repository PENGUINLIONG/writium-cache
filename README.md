# Writium Cache

`writium-cache` provide common cache functionality for implementation of Writium APIs.

To use cache inside an API, first you need to define a cache source so that we can extract data from local files or somewhere else. Any type that implements `CacheSource` can be used. On initializing your API, instantiate `Cache` with your own `CacheSource`, then its ready to go.
