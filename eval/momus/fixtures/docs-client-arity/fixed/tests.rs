mod client;
fn main() { assert_eq!(client::connect("db", 3), "db:3"); }
