//! Closed local launch intent. Remote messages never deserialize into this API.
use serde::{Deserialize, Serialize};
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Solo,
    Host,
    Join,
}
impl Mode {
    pub fn needs_bridge(self) -> bool {
        self != Self::Solo
    }
    pub fn needs_server(self) -> bool {
        self == Self::Host
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Endpoint {
    pub address: String,
    pub port: u16,
}
impl Endpoint {
    pub fn is_auto(&self) -> bool {
        self.port != 0
            && self.address.len() <= 64
            && self
                .address
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_unspecified())
    }
    pub fn valid(&self) -> bool {
        self.port != 0
            && self.address.len() <= 64
            && self
                .address
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| !ip.is_unspecified() && !ip.is_multicast())
    }
}
pub fn opaque(s: &str) -> bool {
    s.len() == 32
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub fn decimal(s: &str) -> bool {
    !s.is_empty()
        && (s == "0" || !s.starts_with('0'))
        && s.bytes().all(|b| b.is_ascii_digit())
        && s.parse::<u64>().is_ok()
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IntentState {
    Start,
    Leave,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LanIntent {
    pub operation_id: String,
    pub generation: String,
    pub state: IntentState,
    pub endpoint: Endpoint,
    pub player_name: String,
    #[serde(default)]
    pub ticket: Option<JoinTicket>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JoinTicket {
    pub room_id: String,
    pub certificate_sha256: String,
    pub join_secret: String,
}
impl LanIntent {
    pub fn valid(&self, mode: Mode) -> bool {
        opaque(&self.operation_id)
            && decimal(&self.generation)
            && (self.endpoint.valid() || (mode == Mode::Host && self.endpoint.is_auto()))
            && !self.player_name.trim().is_empty()
            && self.player_name.chars().count() <= 64
            && !self.player_name.chars().any(char::is_control)
            && match mode {
                Mode::Solo => false,
                Mode::Host => self.ticket.is_none(),
                Mode::Join => self.ticket.as_ref().is_some_and(|t| {
                    opaque(&t.room_id)
                        && opaque(&t.join_secret)
                        && t.certificate_sha256.len() == 64
                        && t.certificate_sha256
                            .bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                }),
            }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LanState {
    Off,
    Starting,
    Connecting,
    Lobby,
    Preparing,
    Active,
    Leaving,
    Error,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LanError {
    InvalidConfig,
    PortBusy,
    HashMismatch,
    StartFailed,
    ProcessFailed,
    ReadyTimeout,
    InvalidStatus,
    VehicleModsMismatch,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LanStatus {
    pub status_sequence: String,
    #[serde(default)]
    pub ticket: Option<ConnectionTicket>,
    pub operation_id: String,
    pub generation: String,
    pub acknowledged_sequence: u64,
    pub state: LanState,
    pub error_code: Option<LanError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectionTicket {
    pub endpoint: Endpoint,
    pub room_id: String,
    pub certificate_sha256: String,
    pub join_secret: String,
}
impl ConnectionTicket {
    pub fn valid(&self) -> bool {
        self.endpoint.valid()
            && opaque(&self.room_id)
            && opaque(&self.join_secret)
            && self.certificate_sha256.len() == 64
            && self
                .certificate_sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }
}
