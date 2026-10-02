# Client quickstart
See [the client API](src/client.rs). connect accepts an endpoint; the example must compile against the current public API. The caller imports connect from the linked API before using this excerpt.

```rust
// connect is imported from the linked client API by the caller.
fn main() {
    let _connection = connect("db");
}
```
