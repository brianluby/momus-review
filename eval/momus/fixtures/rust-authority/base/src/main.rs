mod auth;
fn main() { assert!(!auth::may_delete(&auth::Actor { can_manage: false })); }
