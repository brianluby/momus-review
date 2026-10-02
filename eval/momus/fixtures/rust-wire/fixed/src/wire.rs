pub fn encode(trace: &str) -> String {
    format!("{{\"state\":\"ready\",\"trace\":\"{}\"}}", trace)
}
