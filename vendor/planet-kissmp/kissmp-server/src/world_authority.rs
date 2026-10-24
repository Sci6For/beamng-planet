//! One strict room snapshot, two independent backend interests. No geographic travel cap.
use anyhow::{bail,Result};
use shared::planet::*;
use crate::{planet_session::{ManagedServer,Launch,TransportEvent,atomic_json,random_id},world_resources::*};
use std::{sync::Arc,collections::HashMap,path::{Path,PathBuf},pin::Pin,future::Future};
use tokio::{net::TcpStream,sync::{Mutex,RwLock}};
use serde_json::{Value,json};
pub type ControlHook=Arc<dyn Fn(Id,ControlMessage,Arc<WorldAuthority>)->Pin<Box<dyn Future<Output=Result<()>>+Send>>+Send+Sync>;
pub type FrameFailureHook=Arc<dyn Fn(Id,Id,f64,Arc<WorldAuthority>)->Pin<Box<dyn Future<Output=Result<()>>+Send>>+Send+Sync>;
pub type PrepareHook=Arc<dyn Fn(Id,Id,FrameDefinition,Arc<WorldAuthority>)->Pin<Box<dyn Future<Output=Result<()>>+Send>>+Send+Sync>;
pub type ResourceHook=Arc<dyn Fn(Id,Id,FrameDefinition,Arc<WorldAuthority>)->Pin<Box<dyn Future<Output=Result<()>>+Send>>+Send+Sync>;
struct Interest {write:Mutex<tokio::net::tcp::OwnedWriteHalf>, frame:Mutex<Option<(Id,FrameDefinition)>>, agreed:Mutex<bool>,alias:Mutex<Option<Value>>}
pub struct WorldAuthority {
 pub server:Arc<ManagedServer>, cfg:Launch, host_secret:Id, host:Mutex<Option<Id>>,
 peers:RwLock<HashMap<Id,Arc<Interest>>>, descriptor:RwLock<Option<WorldDescriptor>>,
 activation:Mutex<Option<Value>>, hook:std::sync::RwLock<Option<ControlHook>>,resource_hook:std::sync::RwLock<Option<ResourceHook>>,prepare_hook:std::sync::RwLock<Option<PrepareHook>>,failure_hook:std::sync::RwLock<Option<FrameFailureHook>>,
 pub client_fingerprint:String,pub game_fingerprint:String,
}
pub fn hash_file(path:&Path)->Result<String>{use std::io::Read;let mut f=std::fs::File::open(path)?;let mut h=ring::digest::Context::new(&ring::digest::SHA256);let mut b=vec![0;CHUNK];loop{let n=f.read(&mut b)?;if n==0{break}h.update(&b[..n]);}Ok(h.finish().as_ref().iter().map(|b|format!("{:02x}",b)).collect())}
pub fn fingerprints(profile:&Path,game:&Path)->Result<(String,String)>{
 fn tree(root:&Path,dir:&Path,out:&mut std::collections::BTreeMap<String,String>)->Result<()>{for e in std::fs::read_dir(dir)?{let p=e?.path();if std::fs::symlink_metadata(&p)?.file_type().is_symlink(){bail!("client link")};if p.is_dir(){tree(root,&p,out)?}else{out.insert(p.strip_prefix(root)?.to_string_lossy().replace('\\',"/"),hash_file(&p)?);}}Ok(())}
 let root=profile.join("mods/unpacked");let mut map=std::collections::BTreeMap::new();for name in ["beamng_earth","beamng_planet"]{tree(&root,&root.join(name),&mut map)?};let client=descriptor_bytes_digest(&serde_json::to_vec(&map)?);
 let game_file=game.join("Bin64/BeamNG.drive.x64.exe");
 Ok((client,hash_file(&game_file)?))
}
impl WorldAuthority {
 pub fn new(server:Arc<ManagedServer>,cfg:Launch,host_secret:Id,client:String,game:String)->Arc<Self>{Arc::new(Self{server,cfg,host_secret,host:Mutex::new(None),peers:RwLock::new(HashMap::new()),descriptor:RwLock::new(None),activation:Mutex::new(None),hook:std::sync::RwLock::new(None),resource_hook:std::sync::RwLock::new(None),prepare_hook:std::sync::RwLock::new(None),failure_hook:std::sync::RwLock::new(None),client_fingerprint:client,game_fingerprint:game})}
 pub fn set_control_hook(&self,h:ControlHook){*self.hook.write().unwrap()=Some(h)}
 pub fn set_frame_failure_hook(&self,h:FrameFailureHook){*self.failure_hook.write().unwrap()=Some(h)}
 pub fn set_prepare_hook(&self,h:PrepareHook){*self.prepare_hook.write().unwrap()=Some(h)}
 pub fn set_resource_hook(&self,h:ResourceHook){*self.resource_hook.write().unwrap()=Some(h)}
 pub async fn binding(&self,client:Id)->Option<(Id,FrameDefinition)>{let p=self.peers.read().await.get(&client).cloned()?;let binding=p.frame.lock().await.clone();binding}
 pub async fn clients(&self)->Vec<Id>{self.peers.read().await.keys().copied().collect()}
 pub async fn install(server:Arc<ManagedServer>,cfg:Launch,path:&Path)->Result<Arc<Self>>{
 let secret=random_id();atomic_json(&path.with_file_name("host-world.json"),&json!({"hostSecret":secret}))?;
 let (client,game)=fingerprints(Path::new(&cfg.profile_root),Path::new(&cfg.game_root))?;
 let world=Self::new(server.clone(),cfg,secret,client,game);let(tx,mut rx)=tokio::sync::mpsc::channel(7);server.set_event_sink(tx)?;
 let w=world.clone();tokio::spawn(async move{
 let mut workers:HashMap<Id,tokio::sync::mpsc::Sender<ControlMessage>>=HashMap::new();
 let mut connections:HashMap<Id,quinn::Connection>=HashMap::new();
 while let Some(event)=rx.recv().await{match event{
 TransportEvent::Joined{client,connection}=>{
  connections.insert(client,connection.clone());
  // Six queued + one executing maximum-size control are below the 8MiB peer budget.
  let (tx,mut input)=tokio::sync::mpsc::channel::<ControlMessage>(6);workers.insert(client,tx);
  let ordered=w.clone();let c=connection.clone();tokio::spawn(async move{while let Some(message)=input.recv().await{
   if let Err(e)=ordered.control(client,message).await{log::warn!("world control: {}",e);c.close(1u32.into(),b"resourceRejected");break}
  }});
  let peer=w.clone();tokio::spawn(async move{if let Err(e)=peer.peer(client,connection.clone()).await{log::warn!("world peer {:?}: {}",client,e);let reason=if e.to_string().contains("vehicleModsMismatch"){b"vehicleModsMismatch".as_slice()}else{b"resourceRejected".as_slice()};connection.close(1u32.into(),reason);}});
 },
 TransportEvent::Left{client}=>{if let Some(tx)=workers.remove(&client){let _=tx.try_send(ControlMessage{room:w.server.ticket.room_id,world_epoch:Epoch(1),sequence:Epoch(0),command:Control::Leave{}});}connections.remove(&client);w.release(client).await;},
 TransportEvent::Control{client,message}=>{if workers.get(&client).map_or(true,|tx|tx.try_send(message).is_err()) {if let Some(c)=connections.get(&client){c.close(1u32.into(),b"queueOverflow");}}}
 }}});Ok(world)
 }
 async fn activate(&self)->Result<Value>{let mut activation=self.activation.lock().await;if let Some(v)=&*activation{return Ok(v.clone())}
 let mut tcp=TcpStream::connect(&self.cfg.backend_address).await?;
 write_json(&mut tcp,&json!({"type":"lan_activate","roomId":self.server.ticket.room_id,"revision":"1","clientFingerprint":self.client_fingerprint,"gameFingerprint":self.game_fingerprint,"catalogueHash":self.server.catalogue_digest.read().unwrap().clone()}),65536).await?;
 let value:Value=tokio::time::timeout(std::time::Duration::from_secs(120),read_json(&mut tcp,ENVELOPE_MAX)).await??;
 if value["type"]!="lan_activated"{bail!("backend activation refused")};*activation=Some(value.clone());Ok(value)
 }
 async fn start_world(&self,centre:[f64;3],name:String,setup:Value)->Result<()>{let mut d=self.descriptor.write().await;if d.is_some(){return Ok(())};let a=self.activate().await?;
 let descriptor=WorldDescriptor{room:self.server.ticket.room_id,world_epoch:Epoch(1),backend_fingerprint:descriptor_bytes_digest(&serde_json::to_vec(&a["config"]["generator"])?),native_fingerprint:a["config"]["generator"]["nativeSha256"].as_str().unwrap_or("").into(),client_fingerprint:self.client_fingerprint.clone(),game_fingerprint:self.game_fingerprint.clone(),source_snapshot_id:a["snapshotId"].as_str().unwrap_or("1").into(),physics_config_hash:a["configSha256"].as_str().ok_or_else(||anyhow::anyhow!("backend config digest"))?.into(),location:centre,environment:json!({"physicalConfig":a["config"],"destination":{"lat":centre[0],"lon":centre[1],"name":name,"setup":setup}}),vehicle_catalogue_hash:self.server.catalogue_digest.read().unwrap().clone()};
 *d=Some(descriptor.clone());drop(d);let peers:Vec<_>=self.peers.read().await.keys().copied().collect();for id in peers{self.server.emit_control(id,Control::Descriptor{canonical_json:String::from_utf8(canonical_descriptor(&descriptor).map_err(anyhow::Error::msg)?)?}).await?};Ok(())}
 pub async fn descriptor(&self)->Option<WorldDescriptor>{self.descriptor.read().await.clone()}
 pub async fn subscribe(self:&Arc<Self>,client:Id,centre:[f64;3])->Result<()>{let p=self.peers.read().await.get(&client).cloned().ok_or_else(||anyhow::anyhow!("world peer"))?;if !*p.agreed.lock().await{bail!("descriptor not agreed")};if p.frame.lock().await.is_some(){return Ok(())}
 let lat=centre[0].to_radians();let lon=centre[1].to_radians();let (sl,cl)=lat.sin_cos();let(so,co)=lon.sin_cos();let n=6378137.0/(1.0-6.69437999014e-3*sl*sl).sqrt();let frame=FrameDefinition{frame_id:random_id(),frame_epoch:Epoch(1),origin_wgs84:centre,origin_ecef:[(n+centre[2])*cl*co,(n+centre[2])*cl*so,(n*(1.0-6.69437999014e-3)+centre[2])*sl],enu_basis:[[-so,co,0.],[-sl*co,-sl*so,cl],[cl*co,cl*so,sl]],height_quantum:1./16.};let operation=random_id();let hook=self.prepare_hook.read().unwrap().clone();if let Some(hook)=hook{hook(client,operation,frame.clone(),self.clone()).await?;}self.server.emit_control(client,Control::PrepareFrame{operation,frame:frame.clone(),centre_wgs84:centre}).await?;self.prepare_frame_for(client,operation,frame,centre).await}
 pub async fn prepare_frame(&self,client:Id,frame:FrameDefinition,centre:[f64;3])->Result<()>{self.prepare_frame_for(client,random_id(),frame,centre).await}
 pub async fn prepare_frame_for(&self,client:Id,operation:Id,frame:FrameDefinition,centre:[f64;3])->Result<()>{validate_frame(&frame).map_err(anyhow::Error::msg)?;let p=self.peers.read().await.get(&client).cloned().ok_or_else(||anyhow::anyhow!("world peer"))?;if !*p.agreed.lock().await{bail!("descriptor not agreed")};let mut w=p.write.lock().await;
 write_json(&mut *w,&json!({"type":"lan_bind","roomId":self.server.ticket.room_id,"operation":operation,"frame":{"frame_id":id_hex(frame.frame_id),"epoch":frame.frame_epoch.0,"latitude":frame.origin_wgs84[0],"longitude":frame.origin_wgs84[1],"height":frame.origin_wgs84[2],"quantum":frame.height_quantum}}),65536).await?;
 *p.frame.lock().await=Some((operation,frame.clone()));
 // Project each independent terrain centre into the supplied common ENU frame.
 let rlat=centre[0].to_radians();let rlon=centre[1].to_radians();let nn=6378137.0/(1.-6.69437999014e-3*rlat.sin().powi(2)).sqrt();let ecef=[(nn+centre[2])*rlat.cos()*rlon.cos(),(nn+centre[2])*rlat.cos()*rlon.sin(),(nn*(1.-6.69437999014e-3)+centre[2])*rlat.sin()];let delta=[ecef[0]-frame.origin_ecef[0],ecef[1]-frame.origin_ecef[1],ecef[2]-frame.origin_ecef[2]];let dot=|a:[f64;3]|a.iter().zip(delta).map(|(a,b)|a*b).sum::<f64>();
 write_json(&mut *w,&json!({"type":"prepare_anchor","requestId":id_hex(operation),"lat":frame.origin_wgs84[0],"lon":frame.origin_wgs84[1],"cx":dot(frame.enu_basis[0]),"cy":dot(frame.enu_basis[1])}),65536).await?;Ok(())}
 pub async fn send_earth(&self,client:Id,payload:Value)->Result<()>{validate_earth(&payload)?;let p=self.peers.read().await.get(&client).cloned().ok_or_else(||anyhow::anyhow!("world peer"))?;if !*p.agreed.lock().await||p.frame.lock().await.is_none(){bail!("LAN prepare before agreement/frame")};write_json(&mut *p.write.lock().await,&payload,65536).await?;Ok(())}
 pub async fn release(&self,client:Id){self.peers.write().await.remove(&client);}
 pub async fn control(self:&Arc<Self>,client:Id,message:ControlMessage)->Result<()>{
 if let Control::HostWorld{location,name,setup}=&message.command {
  tokio::time::timeout(std::time::Duration::from_secs(10),async {loop{if self.host.lock().await.is_some(){break};tokio::time::sleep(std::time::Duration::from_millis(10)).await}}).await?;
  if *self.host.lock().await!=Some(client){bail!("host authority required")};validate_host_world(location,name,setup)?;
  return self.start_world(*location,name.clone(),setup.clone()).await
 }
if let Control::DescriptorAgreement{world_digest}=&message.command{let d=self.descriptor().await.ok_or_else(||anyhow::anyhow!("descriptor absent"))?;if canonical_digest(&d).map_err(anyhow::Error::msg)?!=*world_digest{bail!("descriptor mismatch")};let p=self.peers.read().await.get(&client).cloned().ok_or_else(||anyhow::anyhow!("peer"))?;*p.agreed.lock().await=true;return Ok(())}
 let hook=self.hook.read().unwrap().clone();if let Some(hook)=hook{hook(client,message,self.clone()).await}else{bail!("frame coordinator required")}}
 async fn peer(self:Arc<Self>,client:Id,connection:quinn::Connection)->Result<()>{let mut requests=tokio::time::timeout(std::time::Duration::from_secs(120),connection.accept_uni()).await??;
 let open:WorldRequest=read_json(&mut requests,65536).await?;let host=match open{WorldRequest::Open{host_secret,client_fingerprint,game_fingerprint,catalogue_hash}=>{if catalogue_hash!=*self.server.catalogue_digest.read().unwrap(){bail!("vehicleModsMismatch")};if client_fingerprint!=self.client_fingerprint||game_fingerprint!=self.game_fingerprint{bail!("fingerprint mismatch")};host_secret==Some(self.host_secret)},_=>bail!("world stream handshake")};if host{let mut h=self.host.lock().await;if h.is_some(){bail!("host already bound")};*h=Some(client)}
 let mut tcp=TcpStream::connect(&self.cfg.backend_address).await?;write_json(&mut tcp,&json!({"type":"lan_reserve","roomId":self.server.ticket.room_id}),65536).await?;let (mut read,write)=tcp.into_split();let interest=Arc::new(Interest{write:Mutex::new(write),frame:Mutex::new(None),agreed:Mutex::new(false),alias:Mutex::new(None)});self.peers.write().await.insert(client,interest.clone());
 if let Some(d)=self.descriptor().await{self.server.emit_control(client,Control::Descriptor{canonical_json:String::from_utf8(canonical_descriptor(&d).map_err(anyhow::Error::msg)?)?}).await?;}
 let mut resources=connection.open_uni().await?;let profile=PathBuf::from(&self.cfg.profile_root);let c=connection.clone();let failure_world=self.clone();let alias_interest=interest.clone();let reader=tokio::spawn(async move{let mut sent=std::collections::HashSet::new();loop{let mut v:Value=read_json(&mut read,ENVELOPE_MAX).await?;if v["type"]!="lan_envelope"{continue};let mut e:BackendEnvelope=serde_json::from_value(json!({"operation":v["operation"].take(),"frame_id":v["frameId"].take(),"envelope":v["envelope"].take(),"files":v["files"].take()}))?;if e.envelope.payload["type"]=="anchor_error" && e.envelope.payload["error"].as_str().map_or(false,|s|s=="ValueError: terrain relief exceeds shared frame quantum; prepare a new frame") {
 let binding=alias_interest.frame.lock().await.clone().ok_or_else(||anyhow::anyhow!("failed frame binding"))?;
 let hook=failure_world.failure_hook.read().unwrap().clone();if let Some(hook)=hook{hook(client,e.operation,(binding.1.height_quantum*2.).min(1.),failure_world.clone()).await?;continue}
 }
 if e.envelope.payload["type"]=="anchor_ready" {if let Some(alias)=alias_interest.alias.lock().await.take(){e.envelope.payload["requestId"]=alias;}}if let Err(e)=send_envelope(&mut resources,&e,&mut sent,&profile).await{c.close(1u32.into(),b"resourceRejected");return Err(e)}}#[allow(unreachable_code)]Ok::<(),anyhow::Error>(())});
 let result:Result<()>=async{loop{let request:WorldRequest=read_json(&mut requests,65536).await?;match request{
 WorldRequest::Earth{payload}=>{validate_earth(&payload)?;if payload["type"]=="hello"{continue};if payload["type"]=="prepare_anchor"&&interest.frame.lock().await.is_none(){*interest.alias.lock().await=Some(payload["requestId"].clone());let centre=[payload["lat"].as_f64().unwrap_or(0.),payload["lon"].as_f64().unwrap_or(0.),0.];if self.descriptor().await.is_none(){bail!("host must negotiate world before level launch")};
 tokio::time::timeout(std::time::Duration::from_secs(20),async{loop{if *interest.agreed.lock().await{break};tokio::time::sleep(std::time::Duration::from_millis(10)).await}}).await?;self.subscribe(client,centre).await?;self.send_earth(client,json!({"type":"hello","protocol":4,"format":4})).await?;
 }else{self.send_earth(client,payload).await?;}},
 WorldRequest::Ready{operation,room,world_epoch,frame_id,frame_epoch}=>{if room!=self.server.ticket.room_id||world_epoch!=Epoch(1){bail!("ready world")};let binding=interest.frame.lock().await.clone().ok_or_else(||anyhow::anyhow!("ready binding"))?;if binding.0!=operation||binding.1.frame_id!=frame_id||binding.1.frame_epoch!=frame_epoch{bail!("ready frame")};let hook=self.resource_hook.read().unwrap().clone();if let Some(hook)=hook{hook(client,operation,binding.1,self.clone()).await?;}},_=>bail!("duplicate world handshake")}}}.await;
 reader.abort();self.release(client).await;result
 }
}

pub fn validate_host_world(location:&[f64;3],name:&str,setup:&Value)->Result<()>{
 if !location.iter().all(|x|x.is_finite())||location[0].abs()>90.||location[1].abs()>180.||name.len()>512||name.chars().any(|c|c.is_control()){bail!("host destination")}
 let o=setup.as_object().ok_or_else(||anyhow::anyhow!("shared setup"))?;
 if o.keys().any(|k|!["trafficMode","timeOfDay","date","timePlay"].contains(&k.as_str())){bail!("shared setup fields")}
 // The first release excludes NPC traffic: independently generated AI would not be a shared world.
 if o.get("trafficMode").and_then(Value::as_str)!=Some("disabled"){bail!("LAN NPC traffic must be disabled")}
 if !o.get("timeOfDay").and_then(Value::as_f64).map_or(false,|n|(0.0..=1.0).contains(&n)){bail!("host must resolve shared time")}
 if o.get("date").and_then(Value::as_i64).map_or(true,|n|n.abs()>8640000000000){bail!("host shared date")}
 if !["disabled","fast","normal","slow","realtime"].contains(&o.get("timePlay").and_then(Value::as_str).unwrap_or("")){bail!("host must resolve shared timePlay")};Ok(())
}
