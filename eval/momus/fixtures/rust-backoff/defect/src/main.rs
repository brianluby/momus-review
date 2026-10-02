mod retry;
fn main() { assert_eq!(retry::delay_ms(64), 10000); }
