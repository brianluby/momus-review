pub struct Actor { pub can_manage: bool }
pub fn may_delete(actor: &Actor) -> bool { actor.can_manage }
