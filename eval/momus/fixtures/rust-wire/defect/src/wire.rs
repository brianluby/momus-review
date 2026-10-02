pub fn encode(trace: &str) -> String {
    format!("{{\"status\":\"ready\",\"trace\":\"{}\"}}", trace)
}
