use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Gearbox {
    pub lock_coef: f32,
    pub grb_bhv: Option<String>,
    pub grb_idx: i8,
    pub grb_mde: Option<String>,
    pub frzn: bool,
}
