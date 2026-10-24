//! Modified Planet LAN contract. Legacy commands must never be decoded in Planet mode.
use serde::{Deserialize, Deserializer, Serialize, Serializer};
pub const ALPN: &[u8] = b"planet-lan/1";
pub const PROTOCOL_VERSION: u16 = 1;
pub const CONTROL_MAX: usize = 1024 * 1024;
pub const BRIDGE_JSON_MAX: usize = 4 * 1024 * 1024;
pub const RELIABLE_ITEMS: usize = 32;
pub const RELIABLE_BYTES: usize = 8 * 1024 * 1024;
pub const RESOURCE_CHUNK: usize = 256 * 1024;
pub const ARTIFACT_MAX: u64 = 128 * 1024 * 1024;
pub const ACTIVE_TRANSFERS: usize = 2;
pub const SNAPSHOT_HZ: u32 = 30;
pub type ProtocolError = &'static str;
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Epoch(pub u64);
impl Serialize for Epoch {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if s.is_human_readable() {
            s.serialize_str(&self.0.to_string())
        } else {
            s.serialize_u64(self.0)
        }
    }
}
impl<'de> Deserialize<'de> for Epoch {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        if !d.is_human_readable() {
            return u64::deserialize(d).map(Self);
        }
        let s = String::deserialize(d)?;
        if s.is_empty()
            || s.len() > 20
            || !s.bytes().all(|c| c.is_ascii_digit())
            || (s.len() > 1 && s.starts_with('0'))
        {
            return Err(serde::de::Error::custom("invalid decimal epoch"));
        }
        s.parse().map(Self).map_err(serde::de::Error::custom)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Id(pub [u8; 16]);
impl Serialize for Id {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if s.is_human_readable() {
            s.serialize_str(
                &self
                    .0
                    .iter()
                    .map(|b| format!("{:02x}", b))
                    .collect::<String>(),
            )
        } else {
            self.0.serialize(s)
        }
    }
}
impl<'de> Deserialize<'de> for Id {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        if !d.is_human_readable() {
            return <[u8; 16]>::deserialize(d).map(Self);
        }
        let s = String::deserialize(d)?;
        if s.len() != 32
            || !s
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(serde::de::Error::custom("invalid opaque ID"));
        }
        let mut out = [0; 16];
        for i in 0..16 {
            out[i] =
                u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).map_err(serde::de::Error::custom)?
        }
        Ok(Self(out))
    }
}
pub type RoomId = Id;
pub type ConnectionId = Id;
pub type OperationId = Id;
pub type WorldEpoch = Epoch;
pub type FrameEpoch = Epoch;
pub type VehicleEpoch = Epoch;
pub type SourceSeq = Epoch;
pub type ControlSeq = Epoch;
pub fn validate_peer(alpn: &[u8], version: u16) -> Result<(), ProtocolError> {
    if alpn != ALPN || version != PROTOCOL_VERSION {
        Err("protocol_mismatch")
    } else {
        Ok(())
    }
}
/// Host-authoritative canonical descriptor bytes: compact UTF-8 JSON emitted by this
/// pinned Rust serde_json build after sorting object keys recursively. These bytes
/// are transmitted verbatim; other languages MUST hash the bytes, never reserialize.
pub fn canonical_descriptor(descriptor: &WorldDescriptor) -> Result<Vec<u8>, ProtocolError> {
    validate_descriptor(descriptor)?;
    let value = serde_json::to_value(descriptor).map_err(|_| "invalid_descriptor")?;
    let bytes = serde_json::to_vec(&value).map_err(|_| "invalid_descriptor")?;
    if bytes.len() > CONTROL_MAX / 2 {
        return Err("descriptor_too_large");
    }
    Ok(bytes)
}
pub fn canonical_digest(descriptor: &WorldDescriptor) -> Result<String, ProtocolError> {
    Ok(descriptor_bytes_digest(&canonical_descriptor(descriptor)?))
}
pub fn descriptor_bytes_digest(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|x| format!("{:02x}", x))
        .collect()
}
pub fn parse_canonical_descriptor(bytes: &[u8]) -> Result<WorldDescriptor, ProtocolError> {
    if bytes.len() > CONTROL_MAX / 2 {
        return Err("descriptor_too_large");
    }
    let descriptor: WorldDescriptor =
        serde_json::from_slice(bytes).map_err(|_| "invalid_descriptor")?;
    if canonical_descriptor(&descriptor)? != bytes {
        return Err("noncanonical_descriptor");
    }
    Ok(descriptor)
}
fn valid_wgs84(location: &[f64; 3]) -> bool {
    location.iter().all(|x| x.is_finite()) && location[0].abs() <= 90. && location[1].abs() <= 180.
}
pub fn validate_descriptor(descriptor: &WorldDescriptor) -> Result<(), ProtocolError> {
    if !valid_wgs84(&descriptor.location)
        || !is_hash(&descriptor.physics_config_hash)
        || !is_hash(&descriptor.vehicle_catalogue_hash)
    {
        return Err("invalid_descriptor");
    }
    Ok(())
}
pub fn validate_frame(frame: &FrameDefinition) -> Result<(), ProtocolError> {
    if !valid_wgs84(&frame.origin_wgs84)
        || !frame.origin_ecef.iter().all(|x| x.is_finite())
        || !frame.height_quantum.is_finite()
        || frame.height_quantum <= 0.
    {
        return Err("invalid_frame");
    }
    let basis = &frame.enu_basis;
    for i in 0..3 {
        for j in 0..3 {
            let dot: f64 = basis[i].iter().zip(basis[j]).map(|(a, b)| a * b).sum();
            let expected = if i == j { 1. } else { 0. };
            if !dot.is_finite() || (dot - expected).abs() > 1e-6 {
                return Err("invalid_frame");
            }
        }
    }
    let determinant = basis[0][0] * (basis[1][1] * basis[2][2] - basis[1][2] * basis[2][1])
        - basis[0][1] * (basis[1][0] * basis[2][2] - basis[1][2] * basis[2][0])
        + basis[0][2] * (basis[1][0] * basis[2][1] - basis[1][1] * basis[2][0]);
    if (determinant - 1.).abs() > 1e-6 {
        return Err("invalid_frame");
    }
    Ok(())
}
/// Only the helper and its owned local bridge exchange this schema. Never send on QUIC.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalHandshake {
    pub version: u16,
    pub capability: Id,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorldDescriptor {
    pub room: Id,
    pub world_epoch: Epoch,
    pub backend_fingerprint: String,
    pub native_fingerprint: String,
    pub client_fingerprint: String,
    pub game_fingerprint: String,
    pub source_snapshot_id: String,
    pub physics_config_hash: String,
    pub location: [f64; 3],
    pub environment: serde_json::Value,
    pub vehicle_catalogue_hash: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrameDefinition {
    pub frame_id: Id,
    pub frame_epoch: Epoch,
    pub origin_wgs84: [f64; 3],
    pub origin_ecef: [f64; 3],
    pub enu_basis: [[f64; 3]; 3],
    pub height_quantum: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VehicleState {
    pub room: Id,
    pub world_epoch: Epoch,
    pub vehicle_epoch: Epoch,
    pub vehicle_id: Id,
    pub frame_id: Id,
    pub frame_epoch: Epoch,
    pub owner: Id,
    pub source_seq: Epoch,
    pub source_monotonic_time: f64,
    pub position: [f64; 3],
    pub orientation: [f64; 4],
    pub linear_velocity: [f64; 3],
    /// Body local angular velocity radians/sec, right-handed x/y/z.
    pub angular_velocity: [f64; 3],
    pub inputs: [f32; 4],
    pub electrics: crate::vehicle::Electrics,
    pub gearbox: crate::vehicle::Gearbox,
}
impl Default for VehicleState {
    fn default() -> Self {
        Self {
            room: Id::default(),
            world_epoch: Epoch(0),
            vehicle_epoch: Epoch(0),
            vehicle_id: Id::default(),
            frame_id: Id::default(),
            frame_epoch: Epoch(0),
            owner: Id::default(),
            source_seq: Epoch(0),
            source_monotonic_time: 0.,
            position: [0.; 3],
            orientation: [0., 0., 0., 1.],
            linear_velocity: [0.; 3],
            angular_velocity: [0.; 3],
            inputs: [0.; 4],
            electrics: crate::vehicle::Electrics {
                throttle_input: 0.,
                brake_input: 0.,
                clutch: 0.,
                parkingbrake: 0.,
                steering_input: 0.,
            },
            gearbox: crate::vehicle::Gearbox {
                lock_coef: 0.,
                grb_bhv: None,
                grb_idx: 0,
                grb_mde: None,
                frzn: false,
            },
        }
    }
}
pub struct ExpectedIdentity {
    pub room: Id,
    pub world_epoch: Epoch,
    pub vehicle_epoch: Epoch,
    pub owner: Id,
    pub vehicle_id: Id,
    pub frame_id: Id,
    pub frame_epoch: Epoch,
    pub last_sequence: Epoch,
}
pub fn validate_state(s: &VehicleState, e: &ExpectedIdentity) -> Result<(), ProtocolError> {
    if s.room != e.room
        || s.world_epoch != e.world_epoch
        || s.vehicle_epoch != e.vehicle_epoch
        || s.owner != e.owner
        || s.vehicle_id != e.vehicle_id
        || s.frame_id != e.frame_id
        || s.frame_epoch != e.frame_epoch
    {
        return Err("identity_mismatch");
    }
    if s.source_seq <= e.last_sequence {
        return Err("stale_sequence");
    }
    if !s
        .position
        .iter()
        .chain(s.orientation.iter())
        .chain(s.linear_velocity.iter())
        .chain(s.angular_velocity.iter())
        .all(|v| v.is_finite())
        || !s.source_monotonic_time.is_finite()
        || s.source_monotonic_time < 0.
        || !s
            .inputs
            .iter()
            .chain(
                [
                    s.electrics.throttle_input,
                    s.electrics.brake_input,
                    s.electrics.clutch,
                    s.electrics.parkingbrake,
                    s.electrics.steering_input,
                    s.gearbox.lock_coef,
                ]
                .iter(),
            )
            .all(|v| v.is_finite())
    {
        return Err("invalid_state");
    }
    let norm: f64 = s.orientation.iter().map(|v| v * v).sum();
    if (norm - 1.).abs() > 1e-6 {
        return Err("invalid_quaternion");
    }
    Ok(())
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactManifest {
    pub sha256: String,
    pub byte_length: Epoch,
    pub kind: ArtifactKind,
    pub dependencies: Vec<String>,
    pub world_epoch: Epoch,
    pub frame_epoch: Epoch,
    pub logical_key: String,
    pub bounds: [f64; 6],
    pub surface_semantic_digest: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ArtifactKind {
    Collision,
    GroundModel,
    Terrain,
    Visual,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorldEnvelope {
    pub room: Id,
    pub world_epoch: Epoch,
    pub frame_epoch: Epoch,
    pub payload: serde_json::Value,
    pub resources: Vec<ArtifactManifest>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpawnSpec {
    pub vehicle_id: Id,
    pub vehicle_epoch: Epoch,
    pub owner: Id,
    pub model: String,
    pub config: String,
    pub colors: [[f32; 8]; 3],
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum FixedError {
    ProtocolMismatch,
    Unauthorized,
    InvalidState,
    QueueOverflow,
    ResourceRejected,
    Timeout,
    OccupiedPort,
    Aborted,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum SessionStatus {
    Joining,
    Preparing,
    Ready,
    Loading,
    Leaving,
    Closed,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum Control {
    Welcome {
        connection_id: Id,
    },
    Hello {
        version: u16,
    },
    Join {
        room: Id,
        secret: Id,
    },
    Leave {},
    HostWorld {
        location: [f64; 3],
        name: String,
        setup: serde_json::Value,
    },
    Descriptor {
        canonical_json: String,
    },
    DescriptorAgreement {
        world_digest: String,
    },
    Spawn {
        vehicle: SpawnSpec,
    },
    Reset {
        vehicle_id: Id,
        vehicle_epoch: Epoch,
    },
    Remove {
        vehicle_id: Id,
        vehicle_epoch: Epoch,
    },
    PrepareFrame {
        operation: Id,
        frame: FrameDefinition,
        centre_wgs84: [f64; 3],
    },
    LocalReady {
        operation: Id,
    },
    CommitFrame {
        operation: Id,
    },
    FrameApplied {
        operation: Id,
    },
    Resume {
        operation: Id,
    },
    Abort {
        operation: Id,
        reason: FixedError,
    },
    Interest {
        centre_wgs84: [f64; 3],
        radius: f64,
    },
    ResourceRequest {
        sha256: String,
        offset: Epoch,
    },
    Status {
        status: SessionStatus,
    },
    Error {
        reason: FixedError,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlMessage {
    pub room: Id,
    pub world_epoch: Epoch,
    pub sequence: Epoch,
    pub command: Control,
}
pub fn validate_control(bytes: &[u8]) -> Result<Control, ProtocolError> {
    if bytes.len() > CONTROL_MAX {
        return Err("control_too_large");
    }
    let control: Control = serde_json::from_slice(bytes).map_err(|_| "invalid_control")?;
    if let Control::Hello { version, .. } = &control {
        validate_peer(ALPN, *version)?
    }
    match &control {
        Control::Descriptor { canonical_json } => {
            parse_canonical_descriptor(canonical_json.as_bytes())?;
        }
        Control::PrepareFrame {
            frame,
            centre_wgs84,
            ..
        } => {
            validate_frame(frame)?;
            if !valid_wgs84(centre_wgs84) {
                return Err("invalid_centre");
            }
        }

        Control::Interest {
            centre_wgs84,
            radius,
        } if !centre_wgs84.iter().all(|x| x.is_finite())
            || centre_wgs84[0].abs() > 90.
            || centre_wgs84[1].abs() > 180.
            || !radius.is_finite()
            || *radius <= 0. =>
        {
            return Err("invalid_interest")
        }
        Control::ResourceRequest { sha256, .. } if !is_hash(sha256) => return Err("invalid_hash"),
        Control::Spawn { vehicle }
            if vehicle.model.is_empty()
                || vehicle.model.len() > 128
                || !vehicle
                    .model
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_')
                || vehicle.config.len() > CONTROL_MAX / 2
                || !vehicle.colors.iter().flatten().all(|v| v.is_finite()) =>
        {
            return Err("invalid_spawn")
        }
        Control::DescriptorAgreement { world_digest } if !is_hash(world_digest) => {
            return Err("invalid_digest")
        }
        _ => {}
    }
    Ok(control)
}

/// Controls use u32 big-endian length + UTF-8 JSON; never bincode's deserialize_any.
pub fn encode_control(message: &ControlMessage) -> Result<Vec<u8>, ProtocolError> {
    // Validate the typed command before serde_json could turn nonfinite floats into null.
    match &message.command {
        Control::PrepareFrame {
            frame,
            centre_wgs84,
            ..
        } => {
            validate_frame(frame)?;
            if !valid_wgs84(centre_wgs84) {
                return Err("invalid_centre");
            }
        }
        _ => {}
    }
    let command = serde_json::to_vec(&message.command).map_err(|_| "invalid_control")?;
    validate_control(&command)?;
    let payload = serde_json::to_vec(message).map_err(|_| "invalid_control")?;
    if payload.len() > CONTROL_MAX {
        return Err("control_too_large");
    }
    let mut result = (payload.len() as u32).to_be_bytes().to_vec();
    result.extend(payload);
    Ok(result)
}
pub fn decode_control(bytes: &[u8]) -> Result<ControlMessage, ProtocolError> {
    if bytes.len() < 4 {
        return Err("truncated_control");
    }
    let n = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    if n > CONTROL_MAX || bytes.len() != n + 4 {
        return Err("invalid_control_length");
    }
    let message: ControlMessage =
        serde_json::from_slice(&bytes[4..]).map_err(|_| "invalid_control")?;
    let payload = serde_json::to_vec(&message.command).map_err(|_| "invalid_control")?;
    validate_control(&payload)?;
    Ok(message)
}
/// Snapshot datagrams use bincode fixed integer encoding, little endian, with a hard decode cap.
pub fn encode_state(state: &VehicleState) -> Result<Vec<u8>, ProtocolError> {
    use bincode::Options;
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_limit(4096)
        .serialize(state)
        .map_err(|_| "invalid_state")
}
pub fn decode_state(bytes: &[u8]) -> Result<VehicleState, ProtocolError> {
    use bincode::Options;
    if bytes.len() > 4096 {
        return Err("invalid_state_length");
    }
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_limit(4096)
        .reject_trailing_bytes()
        .deserialize(bytes)
        .map_err(|_| "invalid_state")
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Endpoint {
    pub address: std::net::IpAddr,
    pub port: u16,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectionTicket {
    pub endpoint: Endpoint,
    pub room_id: Id,
    pub certificate_sha256: String,
    pub join_secret: Id,
}
pub fn parse_ticket(bytes: &[u8]) -> Result<ConnectionTicket, ProtocolError> {
    if bytes.len() > 1024 {
        return Err("ticket_too_large");
    }
    let ticket: ConnectionTicket = serde_json::from_slice(bytes).map_err(|_| "invalid_ticket")?;
    if ticket.endpoint.port == 0 || !is_hash(&ticket.certificate_sha256) {
        return Err("invalid_ticket");
    }
    Ok(ticket)
}
fn is_hash(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
