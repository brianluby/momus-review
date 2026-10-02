policy_plugin::guarded! { pub fn delete_workspace(id: u64) { storage::delete_with_audit(id); } }
