//! Readiness advertised to guests comes only from the authenticated host's
//! server-issued Resume. Guest requests cannot open their own launch gate.
use shared::planet::*;
#[derive(Default)]
pub struct HostPresence {host:Option<Id>,ready:bool,location:Option<[f64;3]>}
impl HostPresence {
    pub fn is_bound(&self)->bool {self.host.is_some()}
    pub fn bind(&mut self,client:Id) {self.host=Some(client);self.ready=false;self.location=None;}
    pub fn snapshot(&self)->Control {Control::HostState{ready:self.ready,location:self.location}}
    pub fn observe(&mut self,client:Id,c:&Control)->bool {
        if self.host!=Some(client){return false}
        match c {
            Control::PrepareFrame{centre_wgs84,..}=>{self.ready=false;self.location=Some(*centre_wgs84);true},
            Control::Resume{..}=>{self.ready=self.location.is_some();true},
            Control::Abort{..}=>{self.ready=false;true},
            _=>false
        }
    }
    pub fn interest(&mut self,client:Id,location:[f64;3])->bool {
        if self.host!=Some(client)||!location.iter().all(|v|v.is_finite())||location[0].abs()>90.||location[1].abs()>180.{return false}
        self.location=Some(location);self.ready
    }
    pub fn depart(&mut self,client:Id)->bool {
        if self.host!=Some(client){return false}
        self.host=None;self.ready=false;self.location=None;true
    }
}
