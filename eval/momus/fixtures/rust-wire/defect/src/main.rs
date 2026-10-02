mod wire;
fn main() {
    let response = wire::encode("health-123");
    assert!(response.contains("\"state\":\"ready\""));
    assert!(response.contains("health-123"));
}
