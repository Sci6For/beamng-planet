//! Bounded resource wire. It never competes with the persistent control stream.
use anyhow::{bail, Result};
use std::convert::TryInto;use serde::{Deserialize, Serialize};
use shared::planet::*;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
pub const CHUNK: usize = 256 * 1024;
pub const MAX_ARTIFACT: u64 = 128 * 1024 * 1024;
pub const MAX_RESOURCES: usize = 512;
pub const ENVELOPE_MAX: usize = 4 * 1024 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag="kind", deny_unknown_fields)]
pub enum WorldRequest {
 Open { host_secret: Option<Id>, client_fingerprint:String, game_fingerprint:String, catalogue_hash:String },
 Earth { payload:serde_json::Value },
 Ready { operation:Id, room:Id, world_epoch:Epoch, frame_id:Id, frame_epoch:Epoch },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackendEnvelope {pub operation:Id,pub frame_id:Id,pub envelope:WorldEnvelope,pub files:Vec<HostFile>}
#[derive(Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireEnvelope {pub operation:Id,pub frame_id:Id,pub envelope:WorldEnvelope}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostFile {pub sha256:String,pub path:String}
pub async fn read_json<T:serde::de::DeserializeOwned>(r:&mut(impl AsyncRead+Unpin), max:usize)->Result<T>{
 let n=r.read_u32_le().await? as usize; if n==0||n>max {bail!("resource frame length")};let mut b=vec![0;n];r.read_exact(&mut b).await?;Ok(serde_json::from_slice(&b)?)
}
pub async fn write_json<T:Serialize>(w:&mut(impl AsyncWrite+Unpin),v:&T,max:usize)->Result<()>{let b=serde_json::to_vec(v)?;if b.len()>max {bail!("resource frame length")};w.write_u32_le(b.len() as u32).await?;w.write_all(&b).await?;Ok(())}
pub fn validate_manifest(r:&ArtifactManifest)->Result<()>{
 if r.sha256.len()!=64||!r.sha256.bytes().all(|b|b.is_ascii_digit()||(b'a'..=b'f').contains(&b))||r.byte_length.0>MAX_ARTIFACT||r.dependencies.len()>MAX_RESOURCES||!r.bounds.iter().all(|x|x.is_finite()){bail!("invalid artifact manifest")}
 let parts:Vec<_>=r.logical_key.split('.').collect();
 if parts.len()!=2||parts[0]!=r.sha256||!matches!(parts[1],"raw"|"ter"|"png"|"dds"|"jpg"|"jpeg"|"json"|"cdae"|"dae"){bail!("unsafe logical key")};Ok(())
}
/// This closed schema is the only remotely available backend input. Control-plane LAN,
/// cache deletion, URLs, page launching, physical settings and arbitrary files are absent.
pub fn validate_earth(v:&serde_json::Value)->Result<()>{
 let o=v.as_object().ok_or_else(||anyhow::anyhow!("Earth request object"))?;
 let kind=o.get("type").and_then(|x|x.as_str()).unwrap_or("");
 let keys:&[&str]=match kind {
 "hello"=>&["type","protocol","format","modVersion","sessions","slotSession","slotsHeld"],
 "prepare_anchor"=>&["type","requestId","lat","lon","cx","cy"],
 "prepare_terrain"|"commit_terrain"=>&["type","requestId","anchorId","cx","cy"],
 "commit_anchor"=>&["type","anchorId"],
 "pose"=>&["type","anchorId","x","y","z","vx","vy","vz","lat","lon","speed","cam","follow","held","sessions","slotsGone","view"],
 "roads_reset"=>&["type"],_=>bail!("Earth request kind refused")};
 if o.keys().any(|k|!keys.contains(&k.as_str()))||serde_json::to_vec(v)?.len()>64*1024 {bail!("Earth request schema")}
 fn check(v:&serde_json::Value,depth:usize)->bool {if depth>8{return false}match v{serde_json::Value::Array(a)=>a.len()<=512&&a.iter().all(|x|check(x,depth+1)),serde_json::Value::Object(o)=>o.len()<=16&&o.values().all(|x|check(x,depth+1)),serde_json::Value::String(s)=>s.len()<=128&&!s.contains(['\\','/'])&&!s.contains(".."),serde_json::Value::Number(n)=>n.as_f64().map_or(false,|x|x.is_finite()&&x.abs()<1e12),_=>true}}
 // The closed schema bounds the root object (a full pose has 17 permitted
 // fields). Keep the existing smaller object/depth bounds for nested values.
 if !o.values().all(|value|check(value,1)){bail!("Earth field bound")};
 if kind=="prepare_anchor" {let la=o.get("lat").and_then(|x|x.as_f64()).ok_or_else(||anyhow::anyhow!("latitude"))?;let lo=o.get("lon").and_then(|x|x.as_f64()).ok_or_else(||anyhow::anyhow!("longitude"))?;if la.abs()>90.||lo.abs()>180.{bail!("WGS84")}}
 Ok(())
}
/// Streams an already pinned local artifact with fixed scratch memory, checking its hash
/// while sending. Only a completed envelope becomes visible at the receiving proxy.
pub async fn send_envelope(w:&mut quinn::SendStream,e:&BackendEnvelope,sent:&mut std::collections::HashSet<String>,profile:&std::path::Path)->Result<()>{
 if e.envelope.resources.len()>MAX_RESOURCES||e.files.len()!=e.envelope.resources.len(){bail!("resource count")}
 for r in &e.envelope.resources {validate_manifest(r)?;}
 write_json(w,&WireEnvelope{operation:e.operation,frame_id:e.frame_id,envelope:e.envelope.clone()},ENVELOPE_MAX).await?;
 for r in &e.envelope.resources {
  if sent.contains(&r.sha256){w.write_u8(0).await?;continue}
  let f=e.files.iter().find(|f|f.sha256==r.sha256).ok_or_else(||anyhow::anyhow!("missing host artifact"))?;
  let path=std::path::Path::new(&f.path);let canonical=std::fs::canonicalize(path)?;let root=std::fs::canonicalize(profile.join("temp/beamngEarth/planet").join(id_hex(e.envelope.room)))?;
  if !canonical.starts_with(&root)||std::fs::symlink_metadata(path)?.file_type().is_symlink(){bail!("host artifact escape")}
  let mut file=tokio::fs::File::open(path).await?;if file.metadata().await?.len()!=r.byte_length.0{bail!("host length")}
  w.write_u8(1).await?;let mut remaining=r.byte_length.0;let mut b=vec![0;CHUNK];let mut hash=ring::digest::Context::new(&ring::digest::SHA256);
  while remaining>0 {let n=std::cmp::min(remaining,CHUNK as u64) as usize;file.read_exact(&mut b[..n]).await?;hash.update(&b[..n]);w.write_all(&b[..n]).await?;remaining-=n as u64;}
  let actual:String=hash.finish().as_ref().iter().map(|b|format!("{:02x}",b)).collect();if actual!=r.sha256{bail!("host content changed")};sent.insert(r.sha256.clone());
 }
 Ok(())
}

pub fn id_hex(id:Id)->String{id.0.iter().map(|b|format!("{:02x}",b)).collect()}

pub const JSON_ARTIFACT_MAX:u64=8*1024*1024;
pub const EARTH_KINDS:&[&str]=&["hello","status","anchor_ready","terrain_ready","anchor_error","terrain_error","roads_tile","roads_drop","veg_tile","veg_drop","bldfar_tile","bldfar_drop","far_tile","far_remove","far_swap","far_done","far_retex","block_base","sea","ground_ahead","credits","weather","gravity","cache","route_result","where_result","geocode_result","map_tiles_ready","place_previews_ready","random_place_result"];
pub fn validate_data_string(s:&str,paths:&std::collections::HashSet<String>)->Result<()>{
 let low=s.to_ascii_lowercase();
 if s.contains("../")||s.contains("..\\")||s.contains('\\')||s.as_bytes().get(1)==Some(&b':')||[".lua",".js",".cs",".exe",".dll",".py",".bat",".cmd"].iter().any(|x|low.ends_with(x)){bail!("unsafe data reference")}
 if s.starts_with('/') {
  let ext=std::path::Path::new(s).extension().and_then(|s|s.to_str()).unwrap_or("");
  if s.starts_with("/temp/beamngEarth/"){if !paths.contains(s){bail!("unmanifested generated dependency")}}
  else if !s.starts_with("/art/")&&!s.starts_with("/levels/beamng_earth/"){bail!("unknown absolute VFS dependency")}
  else if !matches!(ext,"raw"|"ter"|"png"|"dds"|"jpg"|"jpeg"|"json"|"cdae"|"dae"|""){bail!("unknown packaged dependency")}
 }Ok(())
}
pub fn validate_data_json(v:&serde_json::Value,paths:&std::collections::HashSet<String>,depth:usize)->Result<()>{if depth>32{bail!("JSON nesting")};match v{serde_json::Value::String(s)=>validate_data_string(s,paths)?,serde_json::Value::Array(a)=>{if a.len()>1_000_000{bail!("JSON array bound")};for v in a{validate_data_json(v,paths,depth+1)?}},serde_json::Value::Object(o)=>{if o.len()>1024{bail!("JSON object bound")};for (k,v) in o{validate_data_string(k,paths)?;validate_data_json(v,paths,depth+1)?}},_=>{}}Ok(())}
/// Validate TSShape's msgpack values without allocating geometry blobs. Only UTF8
/// strings allocate (64KiB maximum); binary vectors stream through fixed scratch.
pub fn validate_cdae(r:&mut impl std::io::Read,paths:&std::collections::HashSet<String>)->Result<()>{
 use std::io::Read;
 fn skip(r:&mut impl Read,mut n:u64)->Result<()>{let mut b=[0;8192];while n>0{let k=n.min(b.len() as u64) as usize;r.read_exact(&mut b[..k])?;n-=k as u64;}Ok(())}
 fn len(r:&mut impl Read,n:usize)->Result<u64>{let mut b=[0;8];r.read_exact(&mut b[8-n..])?;Ok(u64::from_be_bytes(b))}
 fn item(r:&mut impl Read,paths:&std::collections::HashSet<String>,depth:usize)->Result<bool>{if depth>32{bail!("CDAE nesting")};let mut marker=[0];if r.read(&mut marker)?==0{return Ok(false)};let m=marker[0];let mut string=None;let mut children=0;
 match m{
 0x00..=0x7f|0xe0..=0xff|0xc0|0xc2|0xc3=>{},0xa0..=0xbf=>string=Some((m&31) as u64),0x90..=0x9f=>children=(m&15) as u64,0x80..=0x8f=>children=(m&15) as u64*2,
 0xd9|0xda|0xdb=>string=Some(len(r,1usize<<((m-0xd9) as usize))?),
 0xc4|0xc5|0xc6=>{let n=len(r,1usize<<((m-0xc4) as usize))?;if n>MAX_ARTIFACT{bail!("CDAE blob")};skip(r,n)?},
 0xcc|0xd0=>skip(r,1)?,0xcd|0xd1=>skip(r,2)?,0xce|0xd2|0xca=>skip(r,4)?,0xcf|0xd3|0xcb=>skip(r,8)?,
 0xdc=>children=len(r,2)?,0xdd=>children=len(r,4)?,0xde=>children=len(r,2)?*2,0xdf=>children=len(r,4)?*2,_=>bail!("unsupported CDAE marker")}
 if let Some(n)=string{if n>65536{bail!("CDAE string bound")};let mut b=vec![0;n as usize];r.read_exact(&mut b)?;validate_data_string(std::str::from_utf8(&b)?,paths)?;}
 if children>1_000_000{bail!("CDAE vector count")};for _ in 0..children{if !item(r,paths,depth+1)?{bail!("truncated CDAE")}}Ok(true)
 }
 let mut header=[0;8];r.read_exact(&mut header)?;if u32::from_le_bytes(header[..4].try_into().unwrap())!=0x007c001f{bail!("CDAE version")};let n=u32::from_le_bytes(header[4..].try_into().unwrap());if n>65536{bail!("CDAE header bound")};while item(r,paths,0)?{}Ok(())
}


