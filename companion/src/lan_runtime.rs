//! LAN jobs are separate owners: dropping these never closes the Earth job.
use crate::{
    backend::{self, BackendError},
    config::Config,
    job::OwnedBackend,
    lan_protocol::*,
    protocol::ErrorCode,
};
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    fs,
    io::Read,
    net::{TcpListener, UdpSocket},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Images {
    bridge_exe: PathBuf,
    bridge_sha256: String,
    server_exe: PathBuf,
    server_sha256: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Launch<'a> {
    request_sequence: u64,
    profile_root: &'a Path,
    cache_root: &'a Path,
    game_root: &'a Path,
    format_version: u32,
    mode: Mode,
    intent: &'a LanIntent,
    capability: &'a str,
    status_path: &'a Path,
    bridge_address: &'static str,
    earth_proxy_address: &'static str,
    backend_address: &'static str,
    max_players: u8,
    max_vehicles_per_client: u8,
    upnp: bool,
    public_listing: bool,
    map: &'static str,
}
pub struct LanRuntime {
    bridge: Option<OwnedBackend>,
    server: Option<OwnedBackend>,
    pub status: Option<LanStatus>,
    directory: Option<PathBuf>,
    deadline: Instant,
    accepted: Option<(Mode, LanIntent)>,
}
impl Default for LanRuntime {
    fn default() -> Self {
        Self {
            bridge: None,
            server: None,
            status: None,
            directory: None,
            deadline: Instant::now(),
            accepted: None,
        }
    }
}
fn fail(code: ErrorCode, msg: &str) -> BackendError {
    BackendError::new(code, msg)
}
fn capability() -> Result<String, BackendError> {
    #[link(name = "advapi32")]
    extern "system" {
        #[link_name = "SystemFunction036"]
        fn random(buffer: *mut u8, len: u32) -> u8;
    }
    let mut b = [0u8; 16];
    if unsafe { random(b.as_mut_ptr(), 16) } == 0 {
        return Err(fail(
            ErrorCode::BackendStartFailed,
            "capability entropy unavailable",
        ));
    }
    Ok(b.iter().map(|v| format!("{v:02x}")).collect())
}
fn image_start(
    exe: &Path,
    hash: &str,
    dir: &Path,
    role: &str,
) -> Result<OwnedBackend, BackendError> {
    let _pin = backend::verified_file(exe, hash)?;
    OwnedBackend::spawn_in(
        exe,
        &[
            OsString::from("--planet-config"),
            dir.join("launch.json").into(),
        ],
        &dir.join(format!("{role}.log")),
        dir,
    )
}
fn start_bridge(images: &Images, dir: &Path) -> Result<OwnedBackend, BackendError> {
    image_start(&images.bridge_exe, &images.bridge_sha256, dir, "bridge")
}
fn start_server(images: &Images, dir: &Path) -> Result<OwnedBackend, BackendError> {
    image_start(&images.server_exe, &images.server_sha256, dir, "server")
}
impl LanRuntime {
    fn close_owned(&mut self) {
        self.bridge.take();
        self.server.take();
        if let Some(d) = self.directory.take() {
            let _ = fs::remove_file(d.join("launch.json"));
        }
    }
    pub fn stop(&mut self) {
        self.close_owned();
        self.status = None;
        self.accepted = None;
    }
    pub fn apply(
        &mut self,
        config: &Config,
        mode: Mode,
        intent: Option<&LanIntent>,
        sequence: u64,
    ) {
        let Some(intent) = intent.filter(|i| mode.needs_bridge() && i.valid(mode)) else {
            self.stop();
            return;
        };
        if intent.state == IntentState::Leave {
            if !self
                .accepted
                .as_ref()
                .is_some_and(|(accepted_mode, accepted)| {
                    *accepted_mode == mode
                        && accepted.operation_id == intent.operation_id
                        && accepted.generation == intent.generation
                })
            {
                return;
            }
            self.close_owned();
            self.status = Some(LanStatus {
                status_sequence: "0".into(),
                ticket: None,
                operation_id: intent.operation_id.clone(),
                generation: intent.generation.clone(),
                acknowledged_sequence: sequence,
                state: LanState::Off,
                error_code: None,
            });
            return;
        }
        if self.status.as_ref().is_some_and(|s| {
            s.operation_id == intent.operation_id && s.generation == intent.generation
        }) {
            if self
                .accepted
                .as_ref()
                .is_some_and(|(m, i)| *m != mode || i != intent)
            {
                self.close_owned();
                let s = self.status.as_mut().unwrap();
                s.state = LanState::Error;
                s.error_code = Some(LanError::InvalidConfig);
            }
            self.status.as_mut().unwrap().acknowledged_sequence = sequence;
            return;
        }
        self.stop();
        self.status = Some(LanStatus {
            status_sequence: "0".into(),
            ticket: None,
            operation_id: intent.operation_id.clone(),
            generation: intent.generation.clone(),
            acknowledged_sequence: sequence,
            state: LanState::Starting,
            error_code: None,
        });
        self.accepted = Some((mode, intent.clone()));
        if let Err(e) = self.start(config, mode, intent) {
            self.close_owned();
            let s = self.status.as_mut().unwrap();
            s.state = LanState::Error;
            s.error_code = Some(match e.code() {
                ErrorCode::PortBusy => LanError::PortBusy,
                ErrorCode::BackendHashMismatch => LanError::HashMismatch,
                _ => LanError::StartFailed,
            });
        }
    }
    fn start(
        &mut self,
        config: &Config,
        mode: Mode,
        intent: &LanIntent,
    ) -> Result<(), BackendError> {
        let mut bytes = Vec::new();
        fs::File::open(config.lan_inventory_path())?
            .take(16385)
            .read_to_end(&mut bytes)?;
        if bytes.len() > 16384 {
            return Err(fail(ErrorCode::InvalidConfig, "oversized LAN inventory"));
        }
        let images: Images = serde_json::from_slice(&bytes)
            .map_err(|_| fail(ErrorCode::InvalidConfig, "missing approved LAN inventory"))?;
        for (exe, hash) in [
            (&images.bridge_exe, &images.bridge_sha256),
            (&images.server_exe, &images.server_sha256),
        ] {
            if hash.len() != 64 {
                return Err(fail(ErrorCode::InvalidConfig, "invalid image pin"));
            }
            let _ = backend::verified_file(exe, hash)?;
        }
        for address in ["127.0.0.1:7894", "127.0.0.1:47615"] {
            drop(
                TcpListener::bind(address)
                    .map_err(|_| fail(ErrorCode::PortBusy, "LAN port occupied"))?,
            );
        }
        if mode.needs_server() {
            let ip = intent.endpoint.address.parse::<std::net::IpAddr>().unwrap();
            drop(
                UdpSocket::bind(std::net::SocketAddr::new(ip, intent.endpoint.port))
                    .map_err(|_| fail(ErrorCode::PortBusy, "host UDP port occupied"))?,
            );
        }
        let dir = config
            .lan_runtime_root()
            .join(format!("{}-{}", intent.operation_id, intent.generation));
        fs::create_dir_all(&dir)?;
        let dir = dir.canonicalize()?;
        let status_path = dir.join("bridge-status.json");
        let cap = capability()?;
        let launch = Launch {
            request_sequence: self.status.as_ref().unwrap().acknowledged_sequence,
            profile_root: &config.profile,
            cache_root: &config.cache_dir,
            game_root: &config.game_root,
            format_version: 1,
            mode,
            intent,
            capability: &cap,
            status_path: &status_path,
            bridge_address: "127.0.0.1:7894",
            earth_proxy_address: "127.0.0.1:47615",
            backend_address: backend::ADDRESS,
            max_players: 2,
            max_vehicles_per_client: 1,
            upnp: false,
            public_listing: false,
            map: "/levels/beamng_earth/info.json",
        };
        let data = serde_json::to_vec(&launch)
            .map_err(|_| fail(ErrorCode::InvalidConfig, "launch encoding failed"))?;
        fs::write(dir.join("launch.json"), data)?;
        self.directory = Some(dir.clone());
        if mode.needs_server() {
            self.server = Some(start_server(&images, &dir)?);
        }
        self.bridge = Some(start_bridge(&images, &dir)?);
        self.deadline = Instant::now() + Duration::from_secs(45);
        Ok(())
    }
    pub fn poll(&mut self) {
        if self.bridge.is_none() {
            return;
        }
        let failed = self
            .bridge
            .iter()
            .chain(self.server.iter())
            .any(|j| !j.is_running().unwrap_or(false));
        let mut error = if failed {
            Some(LanError::ProcessFailed)
        } else {
            None
        };
        if let (Some(dir), Some(old)) = (&self.directory, &self.status) {
            if let Ok(bytes) = (|| -> std::io::Result<Vec<u8>> {
                let mut b = Vec::new();
                fs::File::open(dir.join("bridge-status.json"))?
                    .take(16385)
                    .read_to_end(&mut b)?;
                Ok(b)
            })() {
                if bytes.len() <= 16384 {
                    if let Ok(new) = serde_json::from_slice::<LanStatus>(&bytes) {
                        if (new.state == LanState::Error) == new.error_code.is_some()
                            && new.ticket.as_ref().map_or(true, |t| {
                                t.valid()
                                    && self.accepted.as_ref().is_some_and(|(mode, intent)| {
                                        *mode == Mode::Host
                                            && if intent.endpoint.is_auto() {
                                                t.endpoint.port == intent.endpoint.port
                                                    && t.endpoint
                                                        .address
                                                        .parse::<std::net::IpAddr>()
                                                        .is_ok_and(|ip| !ip.is_loopback())
                                            } else {
                                                t.endpoint == intent.endpoint
                                            }
                                    })
                            })
                            && decimal(&new.status_sequence)
                            && new.status_sequence.parse::<u64>().unwrap()
                                > old.status_sequence.parse::<u64>().unwrap_or(0)
                            && new.operation_id == old.operation_id
                            && new.generation == old.generation
                            && new.acknowledged_sequence <= old.acknowledged_sequence
                        {
                            let ack = old.acknowledged_sequence;
                            self.status = Some(LanStatus {
                                acknowledged_sequence: ack,
                                ..new
                            });
                        }
                    }
                }
            }
        }
        if self
            .status
            .as_ref()
            .is_some_and(|s| matches!(s.state, LanState::Starting | LanState::Connecting))
            && Instant::now() > self.deadline
        {
            error = Some(LanError::ReadyTimeout)
        }
        if self
            .status
            .as_ref()
            .is_some_and(|s| matches!(s.state, LanState::Error | LanState::Off))
        {
            self.close_owned();
            return;
        }
        if let Some(e) = error {
            self.close_owned();
            if let Some(s) = self.status.as_mut() {
                s.state = LanState::Error;
                s.error_code = Some(e);
            }
        }
    }
}

impl Drop for LanRuntime {
    fn drop(&mut self) {
        self.stop();
    }
}
