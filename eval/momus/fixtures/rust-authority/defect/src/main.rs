mod auth;
fn main() {
    let viewer = auth::Actor { can_manage: false };
    assert!(!auth::may_delete(&viewer, "admin"));
}
