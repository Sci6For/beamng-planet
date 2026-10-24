use serde::{Deserialize, Serialize};

pub const FORMAT_VERSION: u32 = 3;
pub const MAX_REQUEST_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Request {
    pub format_version: u32,
    pub session: String,
    pub sequence: u64,
    pub state: RequestState,
    pub level_id: String,
    pub chooser_token: String,
    pub launch_id: String,
    #[serde(default)]
    pub mode: crate::lan_protocol::Mode,
    #[serde(default)]
    pub lan: Option<crate::lan_protocol::LanIntent>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RequestState {
    Preview,
    Launching,
    Active,
    Idle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Observation {
    Baseline,
    Active,
    Idle,
    Ignored,
}

#[derive(Debug, Default)]
pub struct RequestTracker {
    baseline: Option<(String, u64)>,
}

impl RequestTracker {
    /// Only a later sequence from a validated session can authorize activity.
    /// Invalid and partially written files leave the last baseline untouched.
    /// Each file is a complete latest-state snapshot: intermediate preview or
    /// launch records may coalesce between polls. Lua fences token ownership
    /// before publication; this consumer fences schema and session/sequence.
    pub fn observe(&mut self, bytes: &[u8]) -> Observation {
        if bytes.len() > MAX_REQUEST_BYTES {
            return Observation::Ignored;
        }
        let Ok(request) = serde_json::from_slice::<Request>(bytes) else {
            return Observation::Ignored;
        };
        if !request.valid() {
            return Observation::Ignored;
        }
        let observation = match &self.baseline {
            Some((session, sequence)) if session == &request.session => {
                if request.sequence <= *sequence {
                    return Observation::Ignored;
                }
                match request.state {
                    RequestState::Idle => Observation::Idle,
                    _ => Observation::Active,
                }
            }
            _ => Observation::Baseline,
        };
        self.baseline = Some((request.session, request.sequence));
        observation
    }
}

impl Request {
    fn valid(&self) -> bool {
        let bounded = |token: &str| token.chars().count() <= 128;
        let token = |value: &str| !value.trim().is_empty();
        self.format_version == FORMAT_VERSION
            && match self.mode {
                crate::lan_protocol::Mode::Solo => self.lan.is_none(),
                mode => self.lan.as_ref().is_some_and(|l| l.valid(mode)),
            }
            && self.session.len() <= 128
            && !self.session.trim().is_empty()
            && bounded(&self.chooser_token)
            && bounded(&self.launch_id)
            && match self.state {
                RequestState::Idle => {
                    self.level_id.is_empty()
                        && self.chooser_token.is_empty()
                        && self.launch_id.is_empty()
                }
                RequestState::Preview => {
                    self.level_id.is_empty()
                        && token(&self.chooser_token)
                        && self.launch_id.is_empty()
                }
                RequestState::Launching => {
                    self.level_id == "beamng_earth"
                        && token(&self.chooser_token)
                        && token(&self.launch_id)
                }
                RequestState::Active => {
                    self.level_id == "beamng_earth"
                        && ((self.chooser_token.is_empty() && self.launch_id.is_empty())
                            || (token(&self.chooser_token) && token(&self.launch_id)))
                }
            }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Status {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lan: Option<crate::lan_protocol::LanStatus>,
    pub format_version: u32,
    pub session: String,
    pub state: StatusState,
    pub error_code: Option<ErrorCode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StatusState {
    Waiting,
    Starting,
    Ready,
    Stopping,
    Error,
}

/// Closed vocabulary: neither requests nor statuses carry executable commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ErrorCode {
    InvalidConfig,
    ProfileUnavailable,
    GameConflict,
    PortBusy,
    BackendHashMismatch,
    BackendStartFailed,
    BackendReadyTimeout,
    BackendFailed,
    RetryLimit,
}
