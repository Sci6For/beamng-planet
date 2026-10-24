//! Managed Planet transport; does not invoke legacy discovery or scripts.
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use shared::planet::*;
use std::{
    collections::{HashMap as BTreeMap, HashSet as BTreeSet, VecDeque},
    path::Path,
    sync::Arc,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[path = "planet_catalogue.rs"]
pub mod catalogue;
#[path = "host_presence.rs"]
pub mod host_presence;

#[derive(Default)]
pub struct BoundedQueue {
    items: VecDeque<Vec<u8>>,
    bytes: usize,
}
impl BoundedQueue {
    pub fn push(&mut self, b: Vec<u8>) -> Result<()> {
        if self.items.len() >= RELIABLE_ITEMS || self.bytes + b.len() > RELIABLE_BYTES {
            bail!("queueOverflow")
        }
        self.bytes += b.len();
        self.items.push_back(b);
        Ok(())
    }
    pub fn pop(&mut self) -> Option<Vec<u8>> {
        let b = self.items.pop_front()?;
        self.bytes -= b.len();
        Some(b)
    }
    pub fn bytes(&self) -> usize {
        self.bytes
    }
    pub fn front(&self) -> Option<Vec<u8>> {
        self.items.front().cloned()
    }
}
/// Each peer owns its queue and writer. A blocked stream never holds the room lock.
pub struct PeerOutput {
    queue: std::sync::Mutex<BoundedQueue>,
    notify: tokio::sync::Notify,
    pub connection: quinn::Connection,
    sequence: std::sync::atomic::AtomicU64,
}
impl PeerOutput {
    pub fn enqueue(&self, bytes: Vec<u8>) -> Result<()> {
        if let Err(e) = self.queue.lock().unwrap().push(bytes) {
            self.connection.close(1u32.into(), b"queueOverflow");
            return Err(e);
        }
        self.notify.notify_one();
        Ok(())
    }
    async fn write(self: Arc<Self>, mut send: quinn::SendStream) -> Result<()> {
        loop {
            let notified = self.notify.notified();
            let next = self.queue.lock().unwrap().items.front().cloned();
            if let Some(bytes) = next {
                send.write_all(&bytes).await?;
                self.queue.lock().unwrap().pop();
            } else {
                tokio::select! {_=notified=>{},_=self.connection.closed()=>return Ok(())}
            }
        }
    }
}
pub struct ManagedServer {
    pub endpoint: quinn::Endpoint,
    pub ticket: ConnectionTicket,
    pub session: Arc<tokio::sync::Mutex<PlanetSession>>,
    pub catalogue_digest: std::sync::RwLock<String>,
    host_presence: tokio::sync::Mutex<host_presence::HostPresence>,
    peers: Arc<tokio::sync::Mutex<BTreeMap<Id, Arc<PeerOutput>>>>,
    latest: Arc<tokio::sync::Mutex<BTreeMap<Id, Vec<u8>>>>,
    snapshot_gate: tokio::sync::Mutex<()>,
    events: std::sync::Mutex<Option<tokio::sync::mpsc::Sender<TransportEvent>>>,
}
/// Task 5 consumes only authenticated connections. Capability is never included.
#[derive(Clone)]
pub enum TransportEvent {
    Joined {
        client: Id,
        connection: quinn::Connection,
    },
    Control {
        client: Id,
        message: ControlMessage,
    },
    Left {
        client: Id,
    },
}
impl ManagedServer {
    async fn admit_snapshot_with(
        &self,
        client: Id,
        state: &VehicleState,
        bytes: Vec<u8>,
        after_validation: impl std::future::Future<Output = ()>,
    ) -> Result<()> {
        let _publication = self.snapshot_gate.lock().await;
        self.session.lock().await.accept_snapshot(client, state)?;
        after_validation.await;
        self.latest.lock().await.insert(client, bytes);
        Ok(())
    }
    pub async fn advance_world_epoch(&self, next: Epoch) -> Result<()> {
        let _publication = self.snapshot_gate.lock().await;
        self.session.lock().await.advance_world_epoch(next)?;
        self.latest.lock().await.clear();
        Ok(())
    }
    pub async fn set_frame(&self, client: Id, frame_id: Id, epoch: Epoch) -> Result<()> {
        let _publication = self.snapshot_gate.lock().await;
        self.session
            .lock()
            .await
            .set_frame(client, frame_id, epoch)?;
        self.latest.lock().await.remove(&client);
        Ok(())
    }
    pub async fn queued_bytes(&self, client: Id) -> Option<usize> {
        self.peers
            .lock()
            .await
            .get(&client)
            .map(|p| p.queue.lock().unwrap().bytes())
    }
    pub fn set_event_sink(&self, sink: tokio::sync::mpsc::Sender<TransportEvent>) -> Result<()> {
        // Seven maximum-size frames including prefixes remain below 8 MiB.
        if sink.max_capacity() > 7 {
            bail!("world sink capacity must be <=7")
        }
        *self.events.lock().unwrap() = Some(sink);
        Ok(())
    }
    pub async fn bind_host(&self, client: Id) -> Result<()> {
        self.host_presence.lock().await.bind(client);
        Ok(())
    }
    async fn broadcast_host_state(&self, state: Control) {
        let clients: Vec<_> = self.peers.lock().await.keys().copied().collect();
        for id in clients {
            // A guest disconnecting must never abort the host's frame transition.
            let _ = self.emit_control_raw(id, state.clone()).await;
        }
    }
    pub async fn emit_control(&self, client: Id, command: Control) -> Result<()> {
        let mut presence = self.host_presence.lock().await;
        self.emit_control_raw(client, command.clone()).await?;
        if presence.observe(client, &command) {
            self.broadcast_host_state(presence.snapshot()).await;
        } else if presence.is_bound() && matches!(command, Control::Descriptor { .. }) {
            self.emit_control_raw(client, presence.snapshot()).await?;
        }
        Ok(())
    }
    async fn emit_control_raw(&self, client: Id, command: Control) -> Result<()> {
        let session = self.session.lock().await;
        let room = session.room;
        let world_epoch = session.world_epoch;
        drop(session);
        let peers = self.peers.lock().await;
        let peer = peers
            .get(&client)
            .ok_or_else(|| anyhow::anyhow!("unknown peer"))?;
        let seq = peer
            .sequence
            .fetch_update(
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
                |s| s.checked_add(1),
            )
            .map_err(|_| anyhow::anyhow!("sequence exhausted"))?
            + 1;
        peer.enqueue(
            encode_control(&ControlMessage {
                room,
                world_epoch,
                sequence: Epoch(seq),
                command,
            })
            .map_err(anyhow::Error::msg)?,
        )
    }
    fn event(&self, event: TransportEvent) -> Result<()> {
        if let Some(tx) = self.events.lock().unwrap().as_ref() {
            tx.try_send(event)
                .map_err(|_| anyhow::anyhow!("world event queue overflow"))?;
        } else if matches!(event, TransportEvent::Control { .. }) {
            bail!("world integration required")
        }
        Ok(())
    }
    pub async fn send_control(&self, client: Id, message: &ControlMessage) -> Result<()> {
        self.emit_control(client, message.command.clone()).await
    }
    pub fn bind(address: std::net::SocketAddr, catalogue: BTreeSet<String>) -> Result<Self> {
        let cert = rcgen::generate_simple_self_signed(vec!["planet".into()])?;
        let der = cert.serialize_der()?;
        let mut tls = rustls::ServerConfig::builder()
            .with_safe_defaults()
            .with_no_client_auth()
            .with_single_cert(
                vec![rustls::Certificate(der.clone())],
                rustls::PrivateKey(cert.serialize_private_key_der()),
            )?;
        tls.alpn_protocols = vec![ALPN.to_vec()];
        let mut server_config = quinn::ServerConfig::with_crypto(Arc::new(tls));
        let mut transport = quinn::TransportConfig::default();
        transport.keep_alive_interval(Some(std::time::Duration::from_secs(5)));
        transport.max_concurrent_bidi_streams(1u32.into());
        transport.max_concurrent_uni_streams(2u32.into());
        server_config.transport_config(Arc::new(transport));
        let endpoint = quinn::Endpoint::server(server_config, address)?;
        let address = endpoint.local_addr()?;
        let ticket = ConnectionTicket {
            endpoint: Endpoint {
                address: address.ip(),
                port: address.port(),
            },
            room_id: random_id(),
            certificate_sha256: descriptor_bytes_digest(&der),
            join_secret: random_id(),
        };
        let mut session = PlanetSession::new(ticket.room_id);
        session.catalogue = catalogue;
        Ok(Self {
            endpoint,
            ticket,
            catalogue_digest: std::sync::RwLock::new(String::new()),
            host_presence: tokio::sync::Mutex::new(host_presence::HostPresence::default()),
            session: Arc::new(tokio::sync::Mutex::new(session)),
            peers: Arc::new(tokio::sync::Mutex::new(BTreeMap::new())),
            latest: Arc::new(tokio::sync::Mutex::new(BTreeMap::new())),
            snapshot_gate: tokio::sync::Mutex::new(()),
            events: std::sync::Mutex::new(None),
        })
    }
    pub async fn serve(self: Arc<Self>) -> Result<()> {
        let tick = self.clone();
        let publisher = tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_nanos(
                1_000_000_000 / SNAPSHOT_HZ as u64,
            ));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                let _publication = tick.snapshot_gate.lock().await;
                let latest = tick.latest.lock().await.clone();
                let peers = tick.peers.lock().await;
                for (owner, b) in latest {
                    for (id, p) in peers.iter() {
                        if *id != owner {
                            let _ = p.connection.send_datagram(b.clone().into());
                        }
                    }
                }
            }
        });
        while let Some(c) = self.endpoint.accept().await {
            let server = self.clone();
            tokio::spawn(async move {
                if let Err(e) = server.peer(c).await {
                    log::warn!("Planet peer: {}", e);
                }
            });
        }
        publisher.abort();
        Ok(())
    }
    async fn peer(self: Arc<Self>, connecting: quinn::Connecting) -> Result<()> {
        let c = connecting.await?;
        let mut admitted = None;
        let joined=tokio::time::timeout(std::time::Duration::from_secs(10),async {
            let(mut send,mut recv)=c.accept_bi().await?;
            let h=read_control(&mut recv).await?;
            if h.room!=self.ticket.room_id || !matches!(h.command,Control::Hello{version:1}) {bail!("hello")}
            let j=read_control(&mut recv).await?;
            if j.room!=self.ticket.room_id || j.sequence<=h.sequence || !matches!(j.command,Control::Join{room,secret} if room==self.ticket.room_id&&secret==self.ticket.join_secret) {bail!("unauthorized")}
            let id=random_id(); {let mut session=self.session.lock().await;session.admit(id)?;session.last_control.insert(id,j.sequence);} admitted=Some(id);
            // This confirms authentication only. Ready requires later world authority receipts.
            write_control(&mut send,&ControlMessage{room:self.ticket.room_id,world_epoch:Epoch(1),sequence:Epoch(1),command:Control::Welcome{connection_id:id}}).await?;
            Ok::<_,anyhow::Error>((id,send,recv))
        }).await;
        let (id, send, mut recv) = match joined {
            Ok(Ok(v)) => v,
            other => {
                if let Some(id) = admitted {
                    self.session.lock().await.leave(id);
                }
                c.close(1u32.into(), b"unauthorized");
                bail!("join failed: {:?}", other.err())
            }
        };
        let output = Arc::new(PeerOutput {
            queue: std::sync::Mutex::new(BoundedQueue::default()),
            notify: tokio::sync::Notify::new(),
            connection: c.clone(),
            sequence: std::sync::atomic::AtomicU64::new(1),
        });
        self.peers.lock().await.insert(id, output.clone());
        if let Err(e) = self.event(TransportEvent::Joined {
            client: id,
            connection: c.clone(),
        }) {
            self.peers.lock().await.remove(&id);
            self.session.lock().await.leave(id);
            c.close(1u32.into(), b"queueOverflow");
            return Err(e);
        }
        let writer = tokio::spawn(output.write(send));
        let states = self.clone();
        let dc = c.clone();
        let datagrams = tokio::spawn(async move {
            loop {
                let b = dc.read_datagram().await?;
                let state = match decode_state(&b) {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                let _ = states
                    .admit_snapshot_with(id, &state, b.to_vec(), std::future::ready(()))
                    .await;
            }
            #[allow(unreachable_code)]
            Ok::<(), anyhow::Error>(())
        });
        let result: Result<()> = async {
            loop {
                // Dedicated reader: read_exact must never be canceled by a competing select branch.
                let m = read_control(&mut recv).await?;
                if !matches!(
                    m.command,
                    Control::Spawn { .. }
                        | Control::Reset { .. }
                        | Control::Remove { .. }
                        | Control::Leave {}
                ) {
                    self.session.lock().await.accept_control_identity(id, &m)?;
                    if let Control::Interest { centre_wgs84, .. } = &m.command {
                        // Retain the latest host centre for late joiners. Do not
                        // flood a guest still loading with four UI updates/sec.
                        self.host_presence.lock().await.interest(id, *centre_wgs84);
                    }
                    self.event(TransportEvent::Control {
                        client: id,
                        message: m,
                    })?;
                    continue;
                }
                let leaving = matches!(m.command, Control::Leave {});
                let publication = self.snapshot_gate.lock().await;
                let actions = self.session.lock().await.handle(id, m)?;
                if actions.iter().any(|a| {
                    matches!(
                        a.command,
                        Control::Reset { .. } | Control::Remove { .. } | Control::Leave {}
                    )
                }) {
                    self.latest.lock().await.remove(&id);
                }
                drop(publication);
                let peers: Vec<_> = self.peers.lock().await.keys().copied().collect();
                for action in actions {
                    for peer in &peers {
                        let _ = self.emit_control(*peer, action.command.clone()).await;
                    }
                }
                if leaving {
                    break Ok(());
                }
            }
        }
        .await;
        c.close(0u32.into(), b"leaving");
        writer.abort();
        datagrams.abort();
        self.peers.lock().await.remove(&id);
        {
            let mut presence = self.host_presence.lock().await;
            if presence.depart(id) {
                self.broadcast_host_state(presence.snapshot()).await;
            }
        }
        let publication = self.snapshot_gate.lock().await;
        self.latest.lock().await.remove(&id);
        let removed = {
            let mut session = self.session.lock().await;
            let removed = session.owned_removals(id);
            session.leave(id);
            removed
        };
        drop(publication);
        let peers: Vec<_> = self.peers.lock().await.keys().copied().collect();
        for command in removed {
            for peer in &peers {
                let _ = self.emit_control(*peer, command.clone()).await;
            }
        }
        let _ = self.event(TransportEvent::Left { client: id });
        result
    }
}
pub async fn run(path: &Path) -> Result<()> {
    let cfg = Launch::read(path)?;
    if cfg.mode != "host" {
        bail!("server role")
    }
    let (catalogue, digest) = catalogue::installed_catalogue(Path::new(&cfg.game_root), Path::new(&cfg.profile_root))?;
    let server = Arc::new(ManagedServer::bind(
        std::net::SocketAddr::new(
            select_interface(cfg.intent.endpoint.address)?,
            cfg.intent.endpoint.port,
        ),
        catalogue,
    )?);
    *server.catalogue_digest.write().unwrap() = digest;
    atomic_json(
        &path.with_file_name("server-ready.json"),
        &ServerReady {
            ticket: server.ticket.clone(),
        },
    )?;
    let world = crate::world_authority::WorldAuthority::install(server.clone(), cfg, path).await?;
    crate::encounters::install(world.clone());
    server.serve().await
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Intent {
    pub operation_id: Id,
    pub generation: Epoch,
    pub state: String,
    pub endpoint: Endpoint,
    pub player_name: String,
    pub ticket: Option<serde_json::Value>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Launch {
    pub format_version: u32,
    pub request_sequence: u64,
    pub profile_root: String,
    pub cache_root: String,
    pub game_root: String,
    pub mode: String,
    pub intent: Intent,
    pub capability: Id,
    pub status_path: String,
    pub bridge_address: String,
    pub earth_proxy_address: String,
    pub backend_address: String,
    pub max_players: u32,
    pub max_vehicles_per_client: u32,
    pub upnp: bool,
    pub public_listing: bool,
    pub map: String,
}
impl Launch {
    pub fn read(path: &Path) -> Result<Self> {
        if std::fs::metadata(path)?.len() > 16384 {
            bail!("invalidConfig")
        }
        let b = std::fs::read(path)?;
        if b.len() > 16384 {
            bail!("invalidConfig")
        }
        let c: Self = serde_json::from_slice(&b)?;
        if c.format_version != 1
            || !["host", "join"].contains(&c.mode.as_str())
            || c.max_players != 2
            || c.max_vehicles_per_client != 1
            || c.upnp
            || c.public_listing
            || c.bridge_address != "127.0.0.1:7894"
            || c.earth_proxy_address != "127.0.0.1:47615"
            || c.backend_address != "127.0.0.1:47613"
            || c.map != "/levels/beamng_earth/info.json"
        {
            bail!("invalidConfig")
        }
        if c.intent.state != "start"
            || c.intent.player_name.trim().is_empty()
            || c.intent.player_name.chars().count() > 64
            || c.intent.player_name.chars().any(char::is_control)
        {
            bail!("invalidConfig")
        }
        if c.intent.endpoint.port == 0
            || (c.mode == "join" && c.intent.endpoint.address.is_unspecified())
            || c.intent.endpoint.address.is_multicast()
        {
            bail!("invalidConfig")
        }
        for p in [&c.profile_root, &c.cache_root, &c.game_root, &c.status_path] {
            if !Path::new(p).is_absolute() {
                bail!("invalidConfig")
            }
        }
        if c.mode == "join" {
            c.ticket()?;
        } else if c.intent.ticket.is_some() {
            bail!("invalidConfig")
        }
        Ok(c)
    }
    pub fn ticket(&self) -> Result<ConnectionTicket> {
        let v = self
            .intent
            .ticket
            .clone()
            .ok_or_else(|| anyhow::anyhow!("missing ticket"))?;
        let mut o = v
            .as_object()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("ticket"))?;
        if o.len() != 3
            || !["roomId", "certificateSha256", "joinSecret"]
                .iter()
                .all(|k| o.contains_key(*k))
        {
            bail!("invalid ticket")
        }
        o.insert(
            "endpoint".into(),
            serde_json::to_value(&self.intent.endpoint)?,
        );
        parse_ticket(&serde_json::to_vec(&o)?).map_err(anyhow::Error::msg)
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServerReady {
    pub ticket: ConnectionTicket,
}
pub fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > 16384 {
        bail!("status too large")
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    replace_atomic(&tmp, path)?;
    Ok(())
}
#[cfg(not(windows))]
fn replace_atomic(from: &Path, to: &Path) -> Result<()> {
    std::fs::rename(from, to)?;
    Ok(())
}
#[cfg(windows)]
fn replace_atomic(from: &Path, to: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32;
    }
    let from: Vec<_> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<_> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 1 | 8) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}
pub fn random_id() -> Id {
    use rand::RngCore;
    let mut b = [0; 16];
    rand::rngs::OsRng.fill_bytes(&mut b);
    Id(b)
}
pub async fn read_frame(r: &mut (impl tokio::io::AsyncRead + Unpin)) -> Result<Vec<u8>> {
    let n = r.read_u32().await? as usize;
    if n == 0 || n > CONTROL_MAX {
        bail!("invalid_control_length")
    }
    let mut b = vec![0; n + 4];
    b[..4].copy_from_slice(&(n as u32).to_be_bytes());
    r.read_exact(&mut b[4..]).await?;
    Ok(b)
}
pub async fn read_control(r: &mut (impl tokio::io::AsyncRead + Unpin)) -> Result<ControlMessage> {
    decode_control(&read_frame(r).await?).map_err(anyhow::Error::msg)
}
pub async fn write_control(
    w: &mut (impl tokio::io::AsyncWrite + Unpin),
    c: &ControlMessage,
) -> Result<()> {
    w.write_all(&encode_control(c).map_err(anyhow::Error::msg)?)
        .await?;
    Ok(())
}
pub struct PlanetSession {
    pub room: Id,
    pub world_epoch: Epoch,
    peers: BTreeSet<Id>,
    vehicles: BTreeMap<Id, (SpawnSpec, Epoch)>,
    retired: BTreeSet<Id>,
    last_control: BTreeMap<Id, Epoch>,
    last_time: BTreeMap<Id, f64>,
    frames: BTreeMap<Id, (Id, Epoch)>,
    pub catalogue: BTreeSet<String>,
}
impl PlanetSession {
    pub fn new(room: Id) -> Self {
        Self {
            room,
            world_epoch: Epoch(1),
            peers: BTreeSet::new(),
            vehicles: BTreeMap::new(),
            retired: BTreeSet::new(),
            last_control: BTreeMap::new(),
            last_time: BTreeMap::new(),
            frames: BTreeMap::new(),
            catalogue: BTreeSet::new(),
        }
    }
    pub fn admit(&mut self, id: Id) -> Result<()> {
        if self.peers.len() >= 2 {
            bail!("room full")
        };
        self.peers.insert(id);
        Ok(())
    }
    fn owned_removals(&self, client: Id) -> Vec<Control> {
        self.vehicles
            .values()
            .filter(|(v, _)| v.owner == client)
            .map(|(v, _)| Control::Remove {
                vehicle_id: v.vehicle_id,
                vehicle_epoch: v.vehicle_epoch,
            })
            .collect()
    }
    pub fn leave(&mut self, id: Id) {
        self.peers.remove(&id);
        self.last_control.remove(&id);
        self.frames.remove(&id);
        let gone: Vec<_> = self
            .vehicles
            .iter()
            .filter(|(_, (v, _))| v.owner == id)
            .map(|(k, _)| *k)
            .collect();
        for k in gone {
            self.vehicles.remove(&k);
            self.retired.insert(k);
        }
    }
    pub fn set_frame(&mut self, client: Id, frame_id: Id, frame_epoch: Epoch) -> Result<()> {
        if !self.peers.contains(&client)
            || self
                .frames
                .get(&client)
                .map_or(false, |(_, old_epoch)| frame_epoch <= *old_epoch)
        {
            bail!("invalid frame transition")
        }
        self.frames.insert(client, (frame_id, frame_epoch));
        Ok(())
    }
    pub fn advance_world_epoch(&mut self, next: Epoch) -> Result<()> {
        if self.world_epoch.0.checked_add(1) != Some(next.0) {
            bail!("invalid world transition")
        }
        self.world_epoch = next;
        for (_, seq) in self.vehicles.values_mut() {
            *seq = Epoch(0)
        }
        self.last_time.clear();
        self.frames.clear();
        Ok(())
    }
    pub fn accept_control_identity(&mut self, client: Id, m: &ControlMessage) -> Result<()> {
        if !self.peers.contains(&client)
            || m.room != self.room
            || m.world_epoch != self.world_epoch
            || self
                .last_control
                .get(&client)
                .map_or(false, |s| m.sequence <= *s)
        {
            bail!("stale identity")
        }
        self.last_control.insert(client, m.sequence);
        Ok(())
    }
    pub fn handle(&mut self, client: Id, m: ControlMessage) -> Result<Vec<ControlMessage>> {
        if !self.peers.contains(&client)
            || m.room != self.room
            || m.world_epoch != self.world_epoch
            || self
                .last_control
                .get(&client)
                .map_or(false, |s| m.sequence <= *s)
        {
            bail!("stale identity")
        }
        match &m.command {
            Control::Spawn { vehicle } => {
                let config = if vehicle.config.starts_with("vehicles/") {
                    vehicle.config.clone()
                } else {
                    format!(
                        "vehicles/{}/{}{}",
                        vehicle.model,
                        vehicle.config,
                        if vehicle.config.ends_with(".pc") {
                            ""
                        } else {
                            ".pc"
                        }
                    )
                };
                if vehicle.owner != client
                    || vehicle.vehicle_epoch != Epoch(1)
                    || self.retired.contains(&vehicle.vehicle_id)
                    || self.vehicles.contains_key(&vehicle.vehicle_id)
                    || self.vehicles.values().any(|(v, _)| v.owner == client)
                    || !self.catalogue.contains(&config)
                    || config.split('/').nth(1) != Some(vehicle.model.as_str())
                {
                    bail!("invalid spawn")
                }
                self.vehicles
                    .insert(vehicle.vehicle_id, (vehicle.clone(), Epoch(0)));
            }
            Control::Reset {
                vehicle_id,
                vehicle_epoch,
            } => {
                let (v, s) = self
                    .vehicles
                    .get_mut(vehicle_id)
                    .ok_or_else(|| anyhow::anyhow!("vehicle"))?;
                if v.owner != client || v.vehicle_epoch.0.checked_add(1) != Some(vehicle_epoch.0) {
                    bail!("epoch")
                };
                v.vehicle_epoch = *vehicle_epoch;
                *s = Epoch(0);
            }
            Control::Remove {
                vehicle_id,
                vehicle_epoch,
            } => {
                let (v, _) = self
                    .vehicles
                    .get(vehicle_id)
                    .ok_or_else(|| anyhow::anyhow!("vehicle"))?;
                if v.owner != client || *vehicle_epoch != v.vehicle_epoch {
                    bail!("epoch")
                };
                self.vehicles.remove(vehicle_id);
                self.retired.insert(*vehicle_id);
            }
            Control::Leave {} => {
                let removals = self.owned_removals(client);
                self.leave(client);
                return Ok(removals
                    .into_iter()
                    .map(|command| ControlMessage {
                        room: m.room,
                        world_epoch: m.world_epoch,
                        sequence: m.sequence,
                        command,
                    })
                    .collect());
            }
            _ => bail!("world integration required"),
        }
        self.last_control.insert(client, m.sequence);
        Ok(vec![m])
    }
    pub fn accept_snapshot(&mut self, client: Id, s: &VehicleState) -> Result<()> {
        if !self.peers.contains(&client) || client != s.owner {
            bail!("owner")
        }
        let (frame_id, frame_epoch) = self
            .frames
            .get(&client)
            .copied()
            .ok_or_else(|| anyhow::anyhow!("frame not applied"))?;
        let (v, last) = self
            .vehicles
            .get_mut(&s.vehicle_id)
            .ok_or_else(|| anyhow::anyhow!("vehicle"))?;
        validate_state(
            s,
            &ExpectedIdentity {
                room: self.room,
                world_epoch: self.world_epoch,
                vehicle_epoch: v.vehicle_epoch,
                owner: v.owner,
                vehicle_id: v.vehicle_id,
                frame_id,
                frame_epoch,
                last_sequence: *last,
            },
        )
        .map_err(anyhow::Error::msg)?;
        if self
            .last_time
            .get(&s.vehicle_id)
            .map_or(false, |t| s.source_monotonic_time < *t)
        {
            bail!("nonmonotonic timestamp")
        }
        self.last_time.insert(s.vehicle_id, s.source_monotonic_time);
        *last = s.source_seq;
        Ok(())
    }
}

pub fn select_interface(requested: std::net::IpAddr) -> Result<std::net::IpAddr> {
    if !requested.is_unspecified() {
        return Ok(requested);
    }
    let mut addresses = Vec::new();
    for interface in ifcfg::IfCfg::get().map_err(|e| anyhow::anyhow!("interfaces: {:?}", e))? {
        for entry in interface.addresses {
            if let Some(address) = entry.address {
                let ip = address.ip();
                if !ip.is_loopback()
                    && !ip.is_multicast()
                    && !ip.is_unspecified()
                    && ip.is_ipv4() == requested.is_ipv4()
                {
                    addresses.push(ip)
                }
            }
        }
    }
    addresses.sort_by_key(|ip| {
        let rank = match ip {
            std::net::IpAddr::V4(v) if v.is_private() => 0,
            std::net::IpAddr::V4(v) if v.is_link_local() => 1,
            _ => 2,
        };
        (rank, *ip)
    });
    addresses
        .first()
        .copied()
        .ok_or_else(|| anyhow::anyhow!("no usable local interface"))
}

#[cfg(test)]
mod epoch_race_tests {
    use super::*;
    #[tokio::test]
    async fn validated_snapshot_cannot_reinsert_after_world_retirement() {
        let server =
            Arc::new(ManagedServer::bind("127.0.0.1:0".parse().unwrap(), BTreeSet::new()).unwrap());
        let client = Id([11; 16]);
        let vehicle = Id([12; 16]);
        {
            let mut session = server.session.lock().await;
            session.admit(client).unwrap();
            session.set_frame(client, Id([13; 16]), Epoch(1)).unwrap();
            session.vehicles.insert(
                vehicle,
                (
                    SpawnSpec {
                        vehicle_id: vehicle,
                        vehicle_epoch: Epoch(1),
                        owner: client,
                        model: "test".into(),
                        config: "base".into(),
                        colors: [[0.; 8]; 3],
                    },
                    Epoch(0),
                ),
            );
        }
        let state = VehicleState {
            room: server.ticket.room_id,
            world_epoch: Epoch(1),
            vehicle_epoch: Epoch(1),
            vehicle_id: vehicle,
            frame_id: Id([13; 16]),
            frame_epoch: Epoch(1),
            owner: client,
            source_seq: Epoch(1),
            ..VehicleState::default()
        };
        let validated = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let s = server.clone();
        let v = validated.clone();
        let r = release.clone();
        let admission = tokio::spawn(async move {
            s.admit_snapshot_with(client, &state, encode_state(&state).unwrap(), async {
                v.notify_one();
                r.notified().await
            })
            .await
            .unwrap()
        });
        validated.notified().await;
        let s = server.clone();
        let transition =
            tokio::spawn(async move { s.advance_world_epoch(Epoch(2)).await.unwrap() });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let retired_while_admission_inflight = transition.is_finished();
        release.notify_one();
        admission.await.unwrap();
        transition.await.unwrap();
        assert!(
            !retired_while_admission_inflight,
            "retirement crossed admitted snapshot publication"
        );
        assert!(
            server.latest.lock().await.is_empty(),
            "retired snapshot slot was reinserted"
        );
    }
}
