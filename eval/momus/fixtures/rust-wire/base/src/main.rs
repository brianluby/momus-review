mod wire;
fn main() { assert!(wire::encode().contains("\"state\":\"ready\"")); }
