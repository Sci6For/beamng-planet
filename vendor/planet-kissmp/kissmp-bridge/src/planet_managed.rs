//! Fixed-role managed bridge. Local capability never travels on QUIC.
#[path = "world_proxy.rs"]
pub mod world_proxy;
use anyhow::{bail, Result};
use kissmp_server::planet_session::{
    atomic_json, read_control, read_frame, write_control, Launch, ServerReady,
};
use serde::{Deserialize, Serialize};
use shared::planet::*;
use std::{path::Path, sync::Arc};
use tokio::io::AsyncWriteExt;

pub fn certificate_matches(pin: &str, der: &[u8]) -> Result<()> {
    if descriptor_bytes_digest(der) != pin {
        bail!("hashMismatch")
    }
    Ok(())
}
struct Pinned(String);
impl rustls::client::ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        cert: &rustls::Certificate,
        _: &[rustls::Certificate],
        _: &rustls::ServerName,
        _: &mut dyn Iterator<Item = &[u8]>,
        _: &[u8],
        _: std::time::SystemTime,
    ) -> std::result::Result<rustls::client::ServerCertVerified, rustls::Error> {
        certificate_matches(&self.0, &cert.0)
            .map_err(|_| rustls::Error::General("certificate pin mismatch".into()))?;
        Ok(rustls::client::ServerCertVerified::assertion())
    }
}
pub async fn connect(
    ticket: &ConnectionTicket,
) -> Result<(
    quinn::Endpoint,
    quinn::Connection,
    quinn::SendStream,
    quinn::RecvStream,
    ControlMessage,
)> {
    parse_ticket(&serde_json::to_vec(ticket)?).map_err(anyhow::Error::msg)?;
    let bind = if ticket.endpoint.address.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let mut endpoint = quinn::Endpoint::client(bind.parse()?)?;
    let mut tls = rustls::ClientConfig::builder()
        .with_safe_defaults()
        .with_custom_certificate_verifier(Arc::new(Pinned(ticket.certificate_sha256.clone())))
        .with_no_client_auth();
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let mut client_config = quinn::ClientConfig::new(Arc::new(tls));
    let mut transport = quinn::TransportConfig::default();
    transport.keep_alive_interval(Some(std::time::Duration::from_secs(5)));
    transport.max_concurrent_bidi_streams(1u32.into());
    transport.max_concurrent_uni_streams(2u32.into());
    transport.stream_receive_window(quinn::VarInt::from_u32(256 * 1024));
    transport.receive_window(quinn::VarInt::from_u32(8 * 1024 * 1024));
    client_config.transport_config(Arc::new(transport));
    endpoint.set_default_client_config(client_config);
    let connection = endpoint
        .connect(
            std::net::SocketAddr::new(ticket.endpoint.address, ticket.endpoint.port),
            "planet",
        )?
        .await?;
    let (mut send, mut recv) = connection.open_bi().await?;
    for (sequence, command) in [
        (
            1,
            Control::Hello {
                version: PROTOCOL_VERSION,
            },
        ),
        (
            2,
            Control::Join {
                room: ticket.room_id,
                secret: ticket.join_secret,
            },
        ),
    ] {
        write_control(
            &mut send,
            &ControlMessage {
                room: ticket.room_id,
                world_epoch: Epoch(1),
                sequence: Epoch(sequence),
                command,
            },
        )
        .await?;
    }
    let welcome =
        tokio::time::timeout(std::time::Duration::from_secs(10), read_control(&mut recv)).await??;
    if welcome.room != ticket.room_id || !matches!(welcome.command, Control::Welcome { .. }) {
        bail!("invalid welcome")
    }
    Ok((endpoint, connection, send, recv, welcome))
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Status<'a> {
    operation_id: Id,
    generation: Epoch,
    acknowledged_sequence: u64,
    status_sequence: Epoch,
    state: &'a str,
    error_code: Option<&'a str>,
    ticket: Option<&'a ConnectionTicket>,
}
pub struct StatusPublisher {
    cfg: Launch,
    inner: std::sync::Mutex<(u64, Option<ConnectionTicket>)>,
}
impl StatusPublisher {
    pub fn new(cfg: Launch) -> Self {
        Self {
            cfg,
            inner: std::sync::Mutex::new((0, None)),
        }
    }
    pub fn publish(
        &self,
        state: &str,
        error: Option<&str>,
        ticket: Option<&ConnectionTicket>,
    ) -> Result<()> {
        if ![
            "off",
            "starting",
            "connecting",
            "lobby",
            "preparing",
            "active",
            "leaving",
            "error",
        ]
        .contains(&state)
            || (state == "error") != error.is_some()
        {
            bail!("invalid status")
        }
        if let Some(e) = error {
            if ![
                "invalidConfig",
                "portBusy",
                "hashMismatch",
                "vehicleModsMismatch",
                "startFailed",
                "processFailed",
                "readyTimeout",
                "invalidStatus",
            ]
            .contains(&e)
            {
                bail!("invalid status error")
            }
        }
        let mut inner = self.inner.lock().unwrap();
        if let Some(ticket) = ticket {
            if self.cfg.mode != "host" {
                bail!("join cannot replace ticket")
            };
            inner.1 = Some(ticket.clone())
        }
        inner.0 = inner
            .0
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("status sequence exhausted"))?;
        atomic_json(
            Path::new(&self.cfg.status_path),
            &Status {
                operation_id: self.cfg.intent.operation_id,
                generation: self.cfg.intent.generation,
                acknowledged_sequence: self.cfg.request_sequence,
                status_sequence: Epoch(inner.0),
                state,
                error_code: error,
                ticket: inner.1.as_ref(),
            },
        )
    }
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum LocalMessage {
    Control { message: ControlMessage },
    Snapshot { state: VehicleState },
}
#[derive(Default)]
pub(crate) struct SnapshotSlots {
    slots: std::collections::HashMap<Id, VehicleState>,
    vehicles: std::collections::HashMap<Id, (Id, Epoch)>,
    world: Option<Epoch>,
    frame: Option<(Id, Epoch)>,
    pending: Option<(Id, Id, Epoch)>,
    quarantined: bool,
}
impl SnapshotSlots {
    pub(crate) fn accept(&mut self, state: VehicleState) {
        if self.quarantined
            || self.world != Some(state.world_epoch)
            || self.frame != Some((state.frame_id, state.frame_epoch))
            || self.vehicles.get(&state.vehicle_id) != Some(&(state.owner, state.vehicle_epoch))
        {
            return;
        }
        if self
            .slots
            .get(&state.vehicle_id)
            .map_or(true, |old| state.source_seq > old.source_seq)
        {
            self.slots.insert(state.vehicle_id, state);
        }
    }
    pub(crate) fn control(&mut self, message: &ControlMessage) {
        if let Some(world) = self.world {
            if message.world_epoch < world {
                return;
            }
            if message.world_epoch != world {
                self.slots.clear();
                self.quarantined = true;
                self.pending = None;
            }
        }
        self.world = Some(message.world_epoch);
        match &message.command {
            Control::Spawn { vehicle } => {
                if self.vehicles.contains_key(&vehicle.vehicle_id) || self.vehicles.len() < 2 {
                    self.vehicles
                        .insert(vehicle.vehicle_id, (vehicle.owner, vehicle.vehicle_epoch));
                }
            }
            Control::Reset {
                vehicle_id,
                vehicle_epoch,
            } => {
                self.slots.remove(vehicle_id);
                if let Some((_, epoch)) = self.vehicles.get_mut(vehicle_id) {
                    *epoch = *vehicle_epoch
                }
            }
            Control::Remove { vehicle_id, .. } => {
                self.slots.remove(vehicle_id);
                self.vehicles.remove(vehicle_id);
            }
            Control::PrepareFrame {
                operation, frame, ..
            } => {
                self.slots.clear();
                self.pending = Some((*operation, frame.frame_id, frame.frame_epoch));
                self.quarantined = true;
            }
            Control::CommitFrame { operation } => {
                self.slots.clear();
                self.quarantined = true;
                if let Some((pending, frame_id, epoch)) = self.pending {
                    if pending == *operation {
                        self.frame = Some((frame_id, epoch))
                    }
                }
            }
            Control::Resume { operation } => {
                self.slots.clear();
                if let Some((pending, frame_id, epoch)) = self.pending {
                    if pending == *operation {
                        self.frame = Some((frame_id, epoch));
                        self.pending = None;
                        self.quarantined = false
                    }
                }
            }
            Control::Abort { .. } => {
                self.slots.clear();
                self.pending = None;
                self.quarantined = true;
            }
            _ => {}
        }
    }
    pub(crate) fn states(&self) -> Vec<VehicleState> {
        if self.quarantined {
            Vec::new()
        } else {
            self.slots.values().cloned().collect()
        }
    }
}
struct QueuedLocal {
    message: LocalMessage,
    bytes: usize,
    budget: Arc<std::sync::atomic::AtomicUsize>,
}
impl Drop for QueuedLocal {
    fn drop(&mut self) {
        self.budget
            .fetch_sub(self.bytes, std::sync::atomic::Ordering::SeqCst);
    }
}
fn queue_local(
    tx: &tokio::sync::mpsc::Sender<QueuedLocal>,
    budget: &Arc<std::sync::atomic::AtomicUsize>,
    message: LocalMessage,
) -> Result<()> {
    let bytes = serde_json::to_vec(&message)?.len() + 4;
    if bytes > CONTROL_MAX + 4 {
        bail!("local control too large")
    }
    budget
        .fetch_update(
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
            |n| n.checked_add(bytes).filter(|n| *n <= RELIABLE_BYTES),
        )
        .map_err(|_| anyhow::anyhow!("queueOverflow"))?;
    let item = QueuedLocal {
        message,
        bytes,
        budget: budget.clone(),
    };
    tx.try_send(item)
        .map_err(|_| anyhow::anyhow!("queueOverflow"))?;
    Ok(())
}
async fn write_local(w: &mut (impl tokio::io::AsyncWrite + Unpin), m: &LocalMessage) -> Result<()> {
    let b = serde_json::to_vec(m)?;
    if b.len() > CONTROL_MAX {
        bail!("local frame too large")
    }
    w.write_all(&(b.len() as u32).to_be_bytes()).await?;
    w.write_all(&b).await?;
    Ok(())
}
pub async fn run(path: &Path) -> Result<()> {
    let cfg = Launch::read(path)?;
    let status = Arc::new(StatusPublisher::new(cfg.clone()));
    status.publish("starting", None, None)?;
    let result = run_inner(path, &cfg, status.clone()).await;
    if let Err(e) = &result {
        let text = e.to_string();
        let code = if e.is::<tokio::time::error::Elapsed>() {
            "readyTimeout"
        } else {
            session_error_code(&text)
        };
        let _ = status.publish("error", Some(code), None);
    }
    result
}
pub fn session_error_code(text: &str) -> &'static str {
    if text.contains("vehicleModsMismatch") { "vehicleModsMismatch" }
    else if text.contains("10048") || text.contains("in use") { "portBusy" }
    else if text.contains("pin mismatch") || text.contains("hashMismatch") { "hashMismatch" }
    else { "processFailed" }
}
async fn run_inner(path: &Path, cfg: &Launch, status: Arc<StatusPublisher>) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(&cfg.bridge_address).await?;
    let ticket = if cfg.mode == "host" {
        tokio::time::timeout(std::time::Duration::from_secs(35), async {
            loop {
                if std::fs::metadata(path.with_file_name("server-ready.json"))
                    .map_or(false, |m| m.len() <= 16384)
                {
                    if let Ok(b) = std::fs::read(path.with_file_name("server-ready.json")) {
                        if b.len() <= 16384 {
                            if let Ok(r) = serde_json::from_slice::<ServerReady>(&b) {
                                if (cfg.intent.endpoint.address.is_unspecified()
                                    || r.ticket.endpoint.address == cfg.intent.endpoint.address)
                                    && r.ticket.endpoint.port == cfg.intent.endpoint.port
                                    && !r.ticket.endpoint.address.is_unspecified()
                                {
                                    break Ok::<_, anyhow::Error>(r.ticket);
                                }
                            }
                        }
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        })
        .await??
    } else {
        cfg.ticket()?
    };
    status.publish("connecting", None, None)?;
    let (_endpoint, connection, send, mut recv, welcome) =
        tokio::time::timeout(std::time::Duration::from_secs(10), connect(&ticket)).await??;
    status.publish(
        "lobby",
        None,
        if cfg.mode == "host" {
            Some(&ticket)
        } else {
            None
        },
    )?;
    let send = Arc::new(tokio::sync::Mutex::new(send));
    let sequence = Arc::new(std::sync::atomic::AtomicU64::new(2));
    let proxy = world_proxy::start(cfg.clone(), connection.clone(), status.clone()).await?;
    // A bounded receiver retains welcome and network controls until Lua authenticates.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<QueuedLocal>(32);
    let budget = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    queue_local(&tx, &budget, LocalMessage::Control { message: welcome })?;
    let latest = Arc::new(tokio::sync::Mutex::new(SnapshotSlots::default()));
    let dc = connection.clone();
    let dl = latest.clone();
    let datagrams = tokio::spawn(async move {
        loop {
            let b = dc.read_datagram().await?;
            let state = decode_state(&b).map_err(anyhow::Error::msg)?;
            dl.lock().await.accept(state);
        }
        #[allow(unreachable_code)]
        Ok::<(), anyhow::Error>(())
    });
    let cc = connection.clone();
    let cl = latest.clone();
    let control_send = send.clone();
    let control_sequence = sequence.clone();
    let controls = tokio::spawn(async move {
        loop {
            let message = read_control(&mut recv).await?;
            cl.lock().await.control(&message);
            if let Err(error) = proxy
                .control(&message, &control_send, &control_sequence, &status)
                .await {
                cc.close(1u32.into(), session_error_code(&error.to_string()).as_bytes());
                return Err(error);
            }
            if let Err(e) = queue_local(&tx, &budget, LocalMessage::Control { message }) {
                cc.close(1u32.into(), b"queueOverflow");
                return Err(e);
            }
        }
        #[allow(unreachable_code)]
        Ok::<(), anyhow::Error>(())
    });
    let result:Result<()>=async {
        let mut stream=loop {
            let(mut stream,address)=tokio::select!{accepted=listener.accept()=>accepted?,reason=connection.closed()=>return Err(reason.into())};
            if !address.ip().is_loopback(){continue}
            let handshake=tokio::time::timeout(std::time::Duration::from_secs(2),read_frame(&mut stream)).await;
            if let Ok(Ok(bytes))=handshake {if let Ok(h)=serde_json::from_slice::<LocalHandshake>(&bytes[4..]) {if h.version==PROTOCOL_VERSION&&h.capability==cfg.capability {break stream}}}
        };
        let(mut read,mut write)=stream.split();
        let writer=async {let mut tick=tokio::time::interval(std::time::Duration::from_nanos(1_000_000_000/SNAPSHOT_HZ as u64));loop {tokio::select!{
 m=rx.recv()=>{let m=m.ok_or_else(||anyhow::anyhow!("remote closed"))?;write_local(&mut write,&m.message).await?;},
 _=tick.tick()=>{let slots=latest.lock().await;for state in slots.states(){write_local(&mut write,&LocalMessage::Snapshot{state}).await?;}}
 }} #[allow(unreachable_code)]Ok::<(),anyhow::Error>(())};
        let reader=async {loop {let b=read_frame(&mut read).await?;let message:LocalMessage=serde_json::from_slice(&b[4..])?;match message {LocalMessage::Control{message}=>send_sequenced(&send,&sequence,message).await?,LocalMessage::Snapshot{state}=>{let b=encode_state(&state).map_err(anyhow::Error::msg)?;connection.send_datagram(b.into())?;}}} #[allow(unreachable_code)]Ok::<(),anyhow::Error>(())};
        tokio::select!{r=reader=>r,r=writer=>r,reason=connection.closed()=>Err(reason.into())}
    }.await;
    let result = if let Some(reason) = connection.close_reason().filter(|e| e.to_string().contains("vehicleModsMismatch")) { Err(reason.into()) } else { result };
    connection.close(0u32.into(), b"leaving");
    controls.abort();
    datagrams.abort();
    result
}

pub async fn send_sequenced(
    send: &Arc<tokio::sync::Mutex<quinn::SendStream>>,
    seq: &std::sync::atomic::AtomicU64,
    mut m: ControlMessage,
) -> Result<()> {
    let mut s = send.lock().await;
    m.sequence = Epoch(
        seq.fetch_update(
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
            |n| n.checked_add(1),
        )
        .map_err(|_| anyhow::anyhow!("control sequence exhausted"))?
            + 1,
    );
    write_control(&mut *s, &m).await
}
