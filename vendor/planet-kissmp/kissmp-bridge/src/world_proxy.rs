//! Existing protocol4 loopback proxy shared by host and guest. Only verified envelopes
//! enter the Earth socket; control and vehicle datagrams have independent writers.
#[path = "artifact_store.rs"]
pub mod artifact_store;
use super::StatusPublisher;
use anyhow::{bail, Result};
use kissmp_server::{
    planet_session::{catalogue, write_control, Launch},
    world_authority::fingerprints,
    world_resources::*,
};
use serde_json::{json, Value};
use shared::planet::*;
use std::{path::Path, sync::Arc};
use tokio::{
    io::AsyncReadExt,
    sync::{mpsc, Mutex},
};
pub struct Proxy {
    pub binding: Mutex<Option<(Id, FrameDefinition)>>,
    pub fingerprints: Mutex<Option<(String, String, String)>>,
    pub requests: mpsc::Sender<WorldRequest>,
}
impl Proxy {
    pub async fn publish_verified(
        &self,
        wire: &WireEnvelope,
        payload: &Value,
        output: &mut (impl tokio::io::AsyncWrite + Unpin),
        notified: &mut std::collections::HashSet<Id>,
    ) -> Result<bool> {
        // Recheck after ALL artifact reads/verification. Keep the binding receipt lock
        // through game publication and receipt reservation: a new Prepare cannot inherit
        // a completed old transfer or suppress its own later readiness.
        let binding = self.binding.lock().await;
        let current = match binding.as_ref() {
            Some(v) => v,
            None => return Ok(false),
        };
        if wire.operation != current.0
            || wire.frame_id != current.1.frame_id
            || wire.envelope.frame_epoch != current.1.frame_epoch
            || wire.envelope.world_epoch != Epoch(1)
        {
            return Ok(false);
        }
        write_json(output, payload, ENVELOPE_MAX).await?;
        notified.retain(|op| *op == wire.operation);
        if (payload["type"] == "anchor_ready" || payload["type"] == "terrain_ready")
            && notified.insert(wire.operation)
        {
            self.requests
                .try_send(WorldRequest::Ready {
                    operation: wire.operation,
                    room: wire.envelope.room,
                    world_epoch: wire.envelope.world_epoch,
                    frame_id: wire.frame_id,
                    frame_epoch: wire.envelope.frame_epoch,
                })
                .map_err(|_| anyhow::anyhow!("resource receipt queue"))?;
        }
        Ok(true)
    }
    pub async fn control(
        &self,
        m: &ControlMessage,
        send: &Arc<Mutex<quinn::SendStream>>,
        sequence: &std::sync::atomic::AtomicU64,
        status: &StatusPublisher,
    ) -> Result<()> {
        match &m.command {
            Control::Descriptor { canonical_json } => {
                let d = parse_canonical_descriptor(canonical_json.as_bytes())
                    .map_err(anyhow::Error::msg)?;
                let f = self
                    .fingerprints
                    .lock()
                    .await
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("world not opened"))?;
                if d.vehicle_catalogue_hash != f.2 { bail!("vehicleModsMismatch"); }
                if d.client_fingerprint != f.0
                    || d.game_fingerprint != f.1
                    || d.room != m.room
                    || d.world_epoch != m.world_epoch
                {
                    bail!("world fingerprint mismatch")
                };
                super::send_sequenced(
                    send,
                    sequence,
                    ControlMessage {
                        room: m.room,
                        world_epoch: m.world_epoch,
                        sequence: Epoch(3),
                        command: Control::DescriptorAgreement {
                            world_digest: descriptor_bytes_digest(canonical_json.as_bytes()),
                        },
                    },
                )
                .await?;
                status.publish("preparing", None, None)?;
            }
            Control::PrepareFrame {
                operation, frame, ..
            } => {
                validate_frame(frame).map_err(anyhow::Error::msg)?;
                *self.binding.lock().await = Some((*operation, frame.clone()));
            }
            Control::Resume { operation } => {
                if self.binding.lock().await.as_ref().map(|x| x.0) != Some(*operation) {
                    bail!("resume binding")
                };
                status.publish("active", None, None)?;
            }
            Control::Abort { .. } => {
                *self.binding.lock().await = None;
                status.publish("lobby", None, None)?;
            }
            _ => {}
        }
        Ok(())
    }
}
pub async fn start(
    cfg: Launch,
    connection: quinn::Connection,
    status: Arc<StatusPublisher>,
) -> Result<Arc<Proxy>> {
    let listener = tokio::net::TcpListener::bind(&cfg.earth_proxy_address).await?;
    let (tx, mut requests) = mpsc::channel::<WorldRequest>(32);
    let proxy = Arc::new(Proxy {
        binding: Mutex::new(None),
        fingerprints: Mutex::new(None),
        requests: tx.clone(),
    });
    let p = proxy.clone();
    tokio::spawn(async move {
        let result:Result<()>=async{
 let (client,game_hash)=fingerprints(Path::new(&cfg.profile_root),Path::new(&cfg.game_root))?;let(_,cat)=catalogue::installed_catalogue(Path::new(&cfg.game_root),Path::new(&cfg.profile_root))?;*p.fingerprints.lock().await=Some((client.clone(),game_hash.clone(),cat.clone()));
 let host_secret=if cfg.mode=="host"{let path=Path::new(&cfg.status_path).with_file_name("host-world.json");let value:Value=serde_json::from_slice(&std::fs::read(path)?)?;Some(serde_json::from_value(value["hostSecret"].clone())?)}else{None};
 let mut send=connection.open_uni().await?;write_json(&mut send,&WorldRequest::Open{host_secret,client_fingerprint:client,game_fingerprint:game_hash,catalogue_hash:cat},65536).await?;
  let(mut game,address)=listener.accept().await?;if !address.ip().is_loopback(){bail!("Earth proxy loopback")};
let outgoing=async {loop{let r=requests.recv().await.ok_or_else(||anyhow::anyhow!("proxy closed"))?;write_json(&mut send,&r,65536).await?;}#[allow(unreachable_code)]Ok::<(),anyhow::Error>(())};
 let(mut input,output)=game.split();let output=Mutex::new(output);let incoming_output=&output;let pp=p.clone();let cc=connection.clone();let incoming=async move{
 let mut notified=std::collections::HashSet::new();let mut resources=cc.accept_uni().await?;let mut store=artifact_store::ArtifactStore::new(Path::new(&cfg.profile_root),m_room(&cc,&cfg)?)?;loop{
 let wire:WireEnvelope=read_json(&mut resources,ENVELOPE_MAX).await?;let e=&wire.envelope;
 let expected=tokio::time::timeout(std::time::Duration::from_secs(5),async {loop{if let Some(b)=pp.binding.lock().await.clone(){break b};tokio::time::sleep(std::time::Duration::from_millis(5)).await}}).await?;
 let _=expected;if e.resources.len()>MAX_RESOURCES{bail!("resource count")};
 for r in &e.resources {validate_manifest(r)?;let flag=resources.read_u8().await?;if flag==0{if !store.has(r)?{bail!("missing deduplicated artifact")};continue};if flag!=1{bail!("resource flag")};store.stage(r)?;let mut offset=0;let mut b=vec![0;CHUNK];while offset<r.byte_length.0{let n=std::cmp::min(CHUNK as u64,r.byte_length.0-offset) as usize;resources.read_exact(&mut b[..n]).await?;store.accept_chunk(&r.sha256,offset,&b[..n])?;offset+=n as u64;}store.publish(&r.sha256)?;}
 let payload=store.ready(e)?;pp.publish_verified(&wire,&payload,&mut *incoming_output.lock().await,&mut notified).await?;
 }#[allow(unreachable_code)]Ok::<(),anyhow::Error>(())};
 let game_requests=async {loop{
 let payload:Value=read_json(&mut input,65536).await?;
 // The ordinary Earth client also sends local settings and UI requests (cache
 // cap immediately after hello, and periodic place lookups). These must never
 // reach the host backend or tear down an otherwise valid world connection.
 // Reply only with a fixed, bounded kind/code; never echo the request's data.
 if let Some(kind @ ("cache"|"where"|"geocode"|"route"|"map_tiles"|"place_previews"|"random_place"|"open_page"))=payload["type"].as_str(){
  write_json(&mut *output.lock().await,&json!({"type":"request_error","requestType":kind,"errorCode":"lanRequestUnavailable"}),1024).await?;
  continue;
 }
 validate_earth(&payload)?;tx.try_send(WorldRequest::Earth{payload}).map_err(|_|anyhow::anyhow!("Earth queue overflow"))?;
 }#[allow(unreachable_code)]Ok::<(),anyhow::Error>(())};
 tokio::select!{r=outgoing=>r,r=incoming=>r,r=game_requests=>r,reason=connection.closed()=>Err(reason.into())}
 }.await;
        if let Err(e) = result {
            log::warn!("world proxy: {}", e);
            let code=super::session_error_code(&e.to_string());
            connection.close(1u32.into(), code.as_bytes());
            let _ = status.publish("error", Some(code), None);
        }
    });
    Ok(proxy)
}
fn m_room(_connection: &quinn::Connection, cfg: &Launch) -> Result<Id> {
    if cfg.mode == "join" {
        Ok(cfg.ticket()?.room_id)
    } else {
        let v: Value = serde_json::from_slice(&std::fs::read(
            Path::new(&cfg.status_path).with_file_name("server-ready.json"),
        )?)?;
        Ok(serde_json::from_value(v["ticket"]["roomId"].clone())?)
    }
}
