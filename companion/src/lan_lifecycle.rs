use crate::lan_protocol::*;
#[derive(Default)]
pub struct LanLifecycle {
    pub current: Option<LanStatus>,
}
impl LanLifecycle {
    pub fn accept(&mut self, status: LanStatus) -> bool {
        if self.current.as_ref().is_some_and(|old| {
            old.operation_id != status.operation_id
                || old.generation != status.generation
                || old.acknowledged_sequence > status.acknowledged_sequence
        }) {
            return false;
        }
        self.current = Some(status);
        true
    }
    pub fn clear(&mut self) {
        self.current = None;
    }
}
