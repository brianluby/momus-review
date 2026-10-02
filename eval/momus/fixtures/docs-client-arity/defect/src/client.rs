pub fn connect(endpoint: &str, retries: u8) -> String {
    format!("{}:{}", endpoint, retries)
}
