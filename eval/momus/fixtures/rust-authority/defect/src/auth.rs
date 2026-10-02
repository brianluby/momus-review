pub struct Actor { pub can_manage: bool }
pub fn may_delete(actor: &Actor, request_role: &str) -> bool {
    actor.can_manage || request_role == "admin"
}
#[cfg(test)] mod tests {
    #[test] fn manager_allowed() { assert!(super::may_delete(&super::Actor { can_manage: true }, "member")); }
}
