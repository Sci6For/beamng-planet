//! Host-authoritative, content-verified vehicle session overlay. No user files
//! are replaced; only bounded files below this feature's private cache are written.
use super::*;
use crate::{
    planet_session::{atomic_json, Launch},
    world_resources::{read_json, write_json, WorldRequest, CHUNK, ENVELOPE_MAX},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write,
    path::PathBuf,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const MAX_FILES: usize = 16_384;
pub const MAX_FILE: u64 = 2 * 1024 * 1024 * 1024;
pub const MAX_CACHE: u64 = 8 * 1024 * 1024 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileSpec {
    pub path: String,
    pub sha256: String,
    pub byte_length: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub catalogue_hash: String,
    pub stock_hash: String,
    pub files: Vec<FileSpec>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Offer {
    pub catalogue_hash: String,
    pub manifest: Option<Manifest>,
}

fn digest_valid(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub(super) fn safe_path(s: &str) -> bool {
    if s.len() > 512 {
        return false;
    }
    s.split('/').all(|p| {
        !p.is_empty()
            && p != "."
            && p != ".."
            && p.len() <= 255
            && !p.ends_with(['.', ' '])
            && !p
                .chars()
                .any(|c| c.is_control() || "\\:*?\"<>|".contains(c))
            && !matches!(
                p.split('.')
                    .next()
                    .unwrap_or("")
                    .to_ascii_uppercase()
                    .as_str(),
                "CON"
                    | "PRN"
                    | "AUX"
                    | "NUL"
                    | "COM1"
                    | "COM2"
                    | "COM3"
                    | "COM4"
                    | "COM5"
                    | "COM6"
                    | "COM7"
                    | "COM8"
                    | "COM9"
                    | "LPT1"
                    | "LPT2"
                    | "LPT3"
                    | "LPT4"
                    | "LPT5"
                    | "LPT6"
                    | "LPT7"
                    | "LPT8"
                    | "LPT9"
            )
    })
}
fn group(path: &str) -> Result<String> {
    if !safe_path(path) {
        bail!("unsafe vehicle sync path")
    }
    let p: Vec<_> = path.split('/').collect();
    if p.len() == 3 && p[0] == "vehicles" && p[2].ends_with(".pc") {
        return Ok("vehicles".into());
    }
    if p.len() >= 4 && p[0] == "mods" && p[1] == "unpacked" {
        return Ok(p[..3].join("/"));
    }
    if p.len() >= 2
        && p[0] == "mods"
        && p[1] != "unpacked"
        && path.to_ascii_lowercase().ends_with(".zip")
    {
        return Ok(path.into());
    }
    bail!("vehicle sync resource kind")
}
impl Manifest {
    pub fn validate(&self) -> Result<()> {
        if !digest_valid(&self.catalogue_hash)
            || !digest_valid(&self.stock_hash)
            || self.files.len() > MAX_FILES
        {
            bail!("vehicle sync manifest bound")
        }
        let mut total = 0u64;
        let mut names = BTreeSet::new();
        let mut previous = None;
        for f in &self.files {
            group(&f.path)?;
            if !digest_valid(&f.sha256)
                || f.byte_length > MAX_FILE
                || !names.insert(f.path.to_lowercase())
                || previous.map_or(false, |p: &String| p >= &f.path)
            {
                bail!("vehicle sync manifest entry")
            };
            previous = Some(&f.path);
            total = total
                .checked_add(f.byte_length)
                .ok_or_else(|| anyhow::anyhow!("vehicle sync size"))?;
        }
        if total > MAX_CACHE / 2 {
            bail!("vehicle sync download cap")
        }
        let groups = self.groups()?;
        let actual = if groups.is_empty() {
            self.stock_hash.clone()
        } else {
            descriptor_bytes_digest(&serde_json::to_vec(&(self.stock_hash.clone(), groups))?)
        };
        if actual != self.catalogue_hash {
            bail!("vehicle sync manifest hashMismatch")
        }
        Ok(())
    }
    pub fn groups(&self) -> Result<BTreeMap<String, String>> {
        let mut trees: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
        let mut out = BTreeMap::new();
        for f in &self.files {
            let g = group(&f.path)?;
            if g == f.path {
                out.insert(g, f.sha256.clone());
            } else {
                let name = if g == "vehicles" {
                    f.path.clone()
                } else {
                    f.path[g.len() + 1..].to_string()
                };
                trees.entry(g).or_default().insert(name, f.sha256.clone());
            }
        }
        for (g, tree) in trees {
            out.insert(g, descriptor_bytes_digest(&serde_json::to_vec(&tree)?));
        }
        Ok(out)
    }
}

/// This resolves only already validated relative names and rejects links in each
/// existing component, including Windows junctions. The host never supplies an OS path.
fn safe_join(root: &Path, relative: &str, create: bool) -> Result<PathBuf> {
    if !safe_path(relative) {
        bail!("unsafe vehicle sync path")
    };
    reject_link(root)?;
    let parts: Vec<_> = relative.split('/').collect();
    let mut path = root.to_path_buf();
    for (i, part) in parts.iter().enumerate() {
        path.push(part);
        if path.exists() {
            reject_link(&path)?;
        } else if create && i + 1 < parts.len() {
            std::fs::create_dir(&path)?;
        }
    }
    Ok(path)
}
fn file_matches(path: &Path, f: &FileSpec) -> bool {
    reject_link(path).is_ok()
        && std::fs::metadata(path).map_or(false, |m| m.is_file() && m.len() == f.byte_length)
        && crate::world_authority::hash_file(path).map_or(false, |h| h == f.sha256)
}
fn total_bytes(root: &Path) -> Result<u64> {
    if !root.exists() {
        return Ok(0);
    };
    let mut n = 0u64;
    for p in files(root)? {
        n = n
            .checked_add(std::fs::metadata(p)?.len())
            .ok_or_else(|| anyhow::anyhow!("vehicle sync cache size"))?;
        if n > MAX_CACHE {
            bail!("vehicle sync cache full")
        }
    }
    Ok(n)
}
fn progress(cfg: &Launch, state: &str, hash: &str, done: u64, total: u64) -> Result<()> {
    atomic_json(
        &Path::new(&cfg.status_path).with_file_name("vehicle-sync.json"),
        &serde_json::json!({"operationId":cfg.intent.operation_id,"generation":cfg.intent.generation,"state":state,"catalogueHash":hash,"bytesDone":done,"bytesTotal":total}),
    )
}
fn overlay_sizes(manifest: &Manifest, local: &Manifest) -> Result<BTreeMap<String, u64>> {
    let local_groups = local.groups()?;
    let mut sizes = BTreeMap::new();
    let mut index = 0;
    for (g, hash) in manifest.groups()? {
        if g != "vehicles" && local_groups.get(&g) == Some(&hash) {
            continue;
        }
        let files: Vec<_> = manifest
            .files
            .iter()
            .filter(|f| group(&f.path).ok().as_ref() == Some(&g))
            .collect();
        let (name, size) = if g.to_ascii_lowercase().ends_with(".zip") {
            (format!("{}.zip", index), files[0].byte_length)
        } else {
            let size = 22
                + files
                    .iter()
                    .map(|f| {
                        let name_len = if g == "vehicles" {
                            f.path.len()
                        } else {
                            f.path.len() - g.len() - 1
                        };
                        f.byte_length + 76 + 2 * name_len as u64
                    })
                    .sum::<u64>();
            (
                if g == "vehicles" {
                    "configs.zip".into()
                } else {
                    format!("{}.zip", index)
                },
                size,
            )
        };
        sizes.insert(name, size);
        index += 1;
    }
    Ok(sizes)
}
fn overlay_growth(desired: &BTreeMap<String, u64>, existing: &BTreeMap<String, u64>) -> u64 {
    desired
        .iter()
        .map(|(path, len)| len.saturating_sub(*existing.get(path).unwrap_or(&0)))
        .sum()
}
fn startup_scripts(
    root: &Path,
    manifest: &Manifest,
    other: &BTreeMap<String, String>,
) -> Result<bool> {
    fn startup(path: &str) -> bool {
        let p = path.to_ascii_lowercase();
        (p.starts_with("lua/") && !p.starts_with("lua/vehicle/")) || p.starts_with("ui/")
    }
    for (g, hash) in manifest.groups()? {
        if other.get(&g) == Some(&hash) || g == "vehicles" {
            continue;
        };
        if g.to_ascii_lowercase().ends_with(".zip") {
            if super::archive_entries_with_ui(&safe_join(root, &g, false)?, true)?
                .keys()
                .any(|p| startup(p))
            {
                return Ok(true);
            }
        } else if manifest
            .files
            .iter()
            .filter(|f| f.path.starts_with(&format!("{}/", g)))
            .any(|f| startup(&f.path[g.len() + 1..]))
        {
            return Ok(true);
        }
    }
    // A hot mount is added after existing mounts. If it shares a physical asset
    // with another package, that may reverse the host's override precedence.
    let groups = manifest.groups()?;
    if groups.iter().all(|(g, h)| other.get(g) == Some(h)) {
        return Ok(false);
    }
    let mut assets: BTreeMap<String, bool> = BTreeMap::new();
    for (g, hash) in groups {
        if g == "vehicles" {
            continue;
        }
        let changed = other.get(&g) != Some(&hash);
        let names: Vec<String> = if g.to_ascii_lowercase().ends_with(".zip") {
            super::archive_entries_with_ui(&safe_join(root, &g, false)?, true)?
                .into_keys()
                .collect()
        } else {
            manifest
                .files
                .iter()
                .filter(|f| f.path.starts_with(&format!("{}/", g)))
                .map(|f| f.path[g.len() + 1..].to_owned())
                .collect()
        };
        for name in names {
            let key = name.to_ascii_lowercase();
            if !["vehicles/", "art/", "sound/", "lua/"]
                .iter()
                .any(|p| key.starts_with(p))
            {
                continue;
            }
            if let Some(previous_changed) = assets.insert(key, changed) {
                if changed || previous_changed {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

pub async fn send_files(
    w: &mut (impl AsyncWrite + Unpin),
    root: &Path,
    manifest: &Manifest,
    missing: &[String],
) -> Result<()> {
    if missing.len() > MAX_FILES {
        bail!("vehicle sync request bound")
    };
    let mut seen = BTreeSet::new();
    for hash in missing {
        if !seen.insert(hash) {
            bail!("duplicate vehicle sync request")
        };
        let f = manifest
            .files
            .iter()
            .find(|f| &f.sha256 == hash)
            .ok_or_else(|| anyhow::anyhow!("unmanifested vehicle sync request"))?;
        let path = safe_join(root, &f.path, false)?;
        let mut file = std::fs::File::open(path)?;
        if file.metadata()?.len() != f.byte_length {
            bail!("vehicle sync host changed")
        }
        let mut hash = ring::digest::Context::new(&ring::digest::SHA256);
        let mut remaining = f.byte_length;
        let mut b = vec![0; CHUNK];
        while remaining > 0 {
            let n = remaining.min(CHUNK as u64) as usize;
            std::io::Read::read_exact(&mut file, &mut b[..n])?;
            hash.update(&b[..n]);
            w.write_all(&b[..n]).await?;
            remaining -= n as u64;
        }
        let actual: String = hash
            .finish()
            .as_ref()
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect();
        if actual != f.sha256 {
            bail!("vehicle sync host hashMismatch")
        }
    }
    Ok(())
}

/// A tiny stored ZIP writer packages only verified regular files from our own
/// staging tree. No archive is extracted and no shell command is involved.
fn pack(root: &Path, paths: &[(String, PathBuf)], target: &Path) -> Result<()> {
    let mut out = std::fs::File::create(target)?;
    let mut directory = Vec::new();
    let mut offset = 0u64;
    let mut crc_table = [0u32; 256];
    for (i, entry) in crc_table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = (c >> 1) ^ (0xedb88320u32.wrapping_mul(c & 1));
        }
        *entry = c;
    }
    for (name, path) in paths {
        if !safe_path(name) {
            bail!("unsafe overlay name")
        };
        reject_link(path)?;
        if !path.starts_with(root) {
            bail!("overlay path escape")
        };
        let len = std::fs::metadata(path)?.len();
        if len > MAX_FILE || offset + len > u32::MAX as u64 {
            bail!("overlay ZIP size")
        };
        let mut file = std::fs::File::open(path)?;
        let mut crc = 0xffff_ffffu32;
        let mut b = vec![0; CHUNK];
        loop {
            let n = std::io::Read::read(&mut file, &mut b)?;
            if n == 0 {
                break;
            }
            for byte in &b[..n] {
                crc = (crc >> 8) ^ crc_table[((crc ^ *byte as u32) & 255) as usize];
            }
        }
        crc = !crc;
        file.seek(SeekFrom::Start(0))?;
        let mut h = [0u8; 30];
        h[..4].copy_from_slice(b"PK\x03\x04");
        h[4..6].copy_from_slice(&20u16.to_le_bytes());
        h[6..8].copy_from_slice(&0x800u16.to_le_bytes());
        h[14..18].copy_from_slice(&crc.to_le_bytes());
        h[18..22].copy_from_slice(&(len as u32).to_le_bytes());
        h[22..26].copy_from_slice(&(len as u32).to_le_bytes());
        h[26..28].copy_from_slice(&(name.len() as u16).to_le_bytes());
        out.write_all(&h)?;
        out.write_all(name.as_bytes())?;
        std::io::copy(&mut file, &mut out)?;
        let mut c = [0u8; 46];
        c[..4].copy_from_slice(b"PK\x01\x02");
        c[4..6].copy_from_slice(&20u16.to_le_bytes());
        c[6..8].copy_from_slice(&20u16.to_le_bytes());
        c[8..10].copy_from_slice(&0x800u16.to_le_bytes());
        c[16..20].copy_from_slice(&crc.to_le_bytes());
        c[20..24].copy_from_slice(&(len as u32).to_le_bytes());
        c[24..28].copy_from_slice(&(len as u32).to_le_bytes());
        c[28..30].copy_from_slice(&(name.len() as u16).to_le_bytes());
        c[42..46].copy_from_slice(&(offset as u32).to_le_bytes());
        directory.extend(c);
        directory.extend(name.as_bytes());
        offset += 30 + name.len() as u64 + len;
    }
    if paths.len() > u16::MAX as usize || offset + directory.len() as u64 > u32::MAX as u64 {
        bail!("overlay ZIP directory")
    };
    out.write_all(&directory)?;
    let mut end = [0u8; 22];
    end[..4].copy_from_slice(b"PK\x05\x06");
    end[8..10].copy_from_slice(&(paths.len() as u16).to_le_bytes());
    end[10..12].copy_from_slice(&(paths.len() as u16).to_le_bytes());
    end[12..16].copy_from_slice(&(directory.len() as u32).to_le_bytes());
    end[16..20].copy_from_slice(&(offset as u32).to_le_bytes());
    out.write_all(&end)?;
    out.sync_all()?;
    Ok(())
}

pub async fn receive(
    cfg: &Launch,
    r: &mut (impl AsyncRead + Unpin),
    w: &mut (impl AsyncWrite + Unpin),
    local: &Manifest,
) -> Result<String> {
    let offer: Offer = read_json(r, ENVELOPE_MAX).await?;
    let manifest = match offer.manifest {
        None => {
            if offer.catalogue_hash != local.catalogue_hash {
                bail!("vehicle sync missing manifest")
            };
            return Ok(offer.catalogue_hash);
        }
        Some(m) => m,
    };
    manifest.validate()?;
    if manifest.catalogue_hash != offer.catalogue_hash || manifest.stock_hash != local.stock_hash {
        bail!("vehicle sync stock hashMismatch")
    }
    let profile = Path::new(&cfg.profile_root);
    let prefix = format!("temp/beamngEarth/planet-mods/{}", manifest.catalogue_hash);
    let stage = safe_join(profile, &format!("{}/profile/.sentinel", prefix), true)?
        .parent()
        .unwrap()
        .to_path_buf();
    let cache = profile.join("temp/beamngEarth/planet-mods");
    let used = total_bytes(&cache)?;
    let mut required = 0u64;
    let mut missing = Vec::new();
    let mut seen = BTreeSet::new();
    for f in &manifest.files {
        let target = safe_join(&stage, &f.path, true)?;
        if !file_matches(&target, f) {
            required += f.byte_length;
        }
    }
    let desired_overlays = overlay_sizes(&manifest, local)?;
    let mut existing_overlays = BTreeMap::new();
    for name in desired_overlays.keys() {
        let target = safe_join(profile, &format!("{}/mounts/{}", prefix, name), true)?;
        if target.exists() {
            existing_overlays.insert(name.clone(), std::fs::metadata(target)?.len());
        }
    }
    let growth = overlay_growth(&desired_overlays, &existing_overlays);
    if used
        .checked_add(required)
        .and_then(|n| n.checked_add(growth))
        .map_or(true, |n| n > MAX_CACHE)
    {
        bail!("vehicle sync cache full")
    }
    for f in &manifest.files {
        let target = safe_join(&stage, &f.path, true)?;
        if file_matches(&target, f) {
            continue;
        };
        let source = safe_join(profile, &f.path, false)?;
        if file_matches(&source, f) {
            std::fs::copy(source, &target)?;
            continue;
        }
        if seen.insert(f.sha256.clone()) {
            missing.push(f.sha256.clone());
        }
    }
    // Reserve headroom for the mountable copies of unpacked resources as well.
    let download: u64 = missing
        .iter()
        .map(|h| {
            manifest
                .files
                .iter()
                .find(|f| &f.sha256 == h)
                .unwrap()
                .byte_length
        })
        .sum();
    progress(cfg, "downloading", &manifest.catalogue_hash, 0, download)?;
    write_json(
        w,
        &WorldRequest::CatalogueFetch {
            missing: missing.clone(),
        },
        ENVELOPE_MAX,
    )
    .await?;
    let mut done = 0;
    let mut b = vec![0; CHUNK];
    for sha in &missing {
        let f = manifest.files.iter().find(|f| &f.sha256 == sha).unwrap();
        let target = safe_join(&stage, &f.path, true)?;
        let part = target.with_extension("planet-part");
        if part.exists() {
            reject_link(&part)?;
            std::fs::remove_file(&part)?;
        }
        let result: Result<()> = async {
            let mut out = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&part)?;
            let mut hash = ring::digest::Context::new(&ring::digest::SHA256);
            let mut remaining = f.byte_length;
            while remaining > 0 {
                let n = remaining.min(CHUNK as u64) as usize;
                r.read_exact(&mut b[..n]).await?;
                hash.update(&b[..n]);
                out.write_all(&b[..n])?;
                remaining -= n as u64;
            }
            out.sync_all()?;
            let actual: String = hash
                .finish()
                .as_ref()
                .iter()
                .map(|b| format!("{:02x}", b))
                .collect();
            if actual != f.sha256 {
                bail!("vehicle sync hashMismatch")
            };
            drop(out);
            if target.exists() {
                reject_link(&target)?;
                std::fs::remove_file(&target)?;
            }
            std::fs::rename(&part, &target)?;
            Ok(())
        }
        .await;
        if let Err(e) = result {
            let _ = std::fs::remove_file(&part);
            return Err(e);
        }
        for duplicate in manifest
            .files
            .iter()
            .filter(|x| x.sha256 == f.sha256 && x.path != f.path)
        {
            let p = safe_join(&stage, &duplicate.path, true)?;
            std::fs::copy(&target, p)?;
        }
        done += f.byte_length;
        progress(cfg, "downloading", &manifest.catalogue_hash, done, download)?;
    }
    // Rebuild the catalogue from actual staged bytes, including ZIP members and
    // every JBeam/script/material dependency. A declared digest alone is never enough.
    let (config_files, verified) = super::snapshot(Path::new(&cfg.game_root), &stage)?;
    if verified.catalogue_hash != manifest.catalogue_hash {
        bail!("vehicle sync staged hashMismatch")
    }
    let local_groups = local.groups()?;
    let host_groups = manifest.groups()?;
    let mut mounts = Vec::new();
    let mut unmounts = Vec::new();
    let mut overrides = BTreeMap::new();
    for (g, h) in &local_groups {
        if g != "vehicles" && host_groups.get(g) != Some(h) {
            unmounts.push(format!(
                "/{}{}",
                g,
                if g.starts_with("mods/unpacked/") {
                    "/"
                } else {
                    ""
                }
            ));
        }
    }
    for (g, h) in &host_groups {
        if g == "vehicles" {
            let mut paths = Vec::new();
            for f in manifest
                .files
                .iter()
                .filter(|f| f.path.starts_with("vehicles/"))
            {
                overrides.insert(f.path.clone(), format!("/{}/profile/{}", prefix, f.path));
                paths.push((f.path.clone(), stage.join(&f.path)));
            }
            let rel = format!("{}/mounts/configs.zip", prefix);
            let target = safe_join(profile, &rel, true)?;
            pack(&stage, &paths, &target)?;
            mounts.push(serde_json::json!({"path":format!("/{}",rel),"kind":"zip"}));
            continue;
        }
        if local_groups.get(g) == Some(h) {
            continue;
        }
        let mount = if g.to_ascii_lowercase().ends_with(".zip") {
            let rel = format!("{}/mounts/{}.zip", prefix, mounts.len());
            let target = safe_join(profile, &rel, true)?;
            std::fs::copy(safe_join(&stage, g, false)?, &target)?;
            format!("/{}", rel)
        } else {
            let rel = format!("{}/mounts/{}.zip", prefix, mounts.len());
            let target = safe_join(profile, &rel, true)?;
            let paths: Vec<_> = manifest
                .files
                .iter()
                .filter(|f| group(&f.path).ok().as_ref() == Some(g))
                .map(|f| (f.path[g.len() + 1..].to_string(), stage.join(&f.path)))
                .collect();
            pack(&stage, &paths, &target)?;
            format!("/{}", rel)
        };
        mounts.push(serde_json::json!({"path":mount,"kind":"zip"}));
    }
    // A guest-only personal file can shadow a host packaged setup. Without a
    // verified packaged-config extractor we must preserve that file and refuse
    // admission, rather than acknowledge different physical parts under one name.
    let personal_shadow=local.files.iter().any(|f|f.path.starts_with("vehicles/")&&config_files.contains(&f.path)&&!overrides.contains_key(&f.path));
    let restart_required = startup_scripts(&stage, &manifest, &local_groups)?
        || startup_scripts(profile, local, &host_groups)? || personal_shadow;
    let plan = serde_json::json!({"operationId":cfg.intent.operation_id,"generation":cfg.intent.generation,"catalogueHash":manifest.catalogue_hash,"state":"mountRequired","mounts":mounts,"unmounts":unmounts,"configOverrides":overrides,"configFiles":config_files,"restartRequired":restart_required,"bytesDone":done,"bytesTotal":download});
    let receipt = Path::new(&cfg.status_path).with_file_name("vehicle-sync-applied.json");
    if receipt.exists() {
        reject_link(&receipt)?;
        std::fs::remove_file(&receipt)?;
    }
    atomic_json(
        &Path::new(&cfg.status_path).with_file_name("vehicle-sync.json"),
        &plan,
    )?;
    tokio::time::timeout(std::time::Duration::from_secs(180), async {
        loop {
            if std::fs::metadata(&receipt).map_or(false, |m| m.len() <= 16384) {
                if let Ok(bytes) = std::fs::read(&receipt) {
                    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                        if v["operationId"] == plan["operationId"]
                            && v["generation"] == plan["generation"]
                            && v["catalogueHash"] == plan["catalogueHash"]
                        {
                            if v["ok"] == true {
                                break Ok::<(), anyhow::Error>(());
                            } else {
                                bail!("vehicle sync mount/restart required")
                            }
                        }
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await??;
    progress(cfg, "ready", &manifest.catalogue_hash, done, download)?;
    Ok(manifest.catalogue_hash)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cached_three_gib_overlay_does_not_reserve_its_bytes_twice() {
        let sizes = BTreeMap::from([("0.zip".to_string(), 3 * 1024 * 1024 * 1024)]);
        let existing = sizes.clone();
        let used = 6 * 1024 * 1024 * 1024;
        assert_eq!(overlay_growth(&sizes, &existing), 0);
        assert!(used + overlay_growth(&sizes, &existing) <= MAX_CACHE);
        let other = BTreeMap::from([("old.zip".to_string(), 3 * 1024 * 1024 * 1024)]);
        assert_eq!(
            overlay_growth(&sizes, &other),
            3 * 1024 * 1024 * 1024,
            "unrelated cached files cannot cancel new growth"
        );
    }
    #[test]
    fn stored_overlay_has_valid_crc_and_canonical_vehicle_members() {
        let root = std::env::temp_dir().join(format!(
            "planet-overlay-{}",
            crate::world_resources::id_hex(crate::planet_session::random_id())
        ));
        std::fs::create_dir(&root).unwrap();
        let source = root.join("source.pc");
        std::fs::write(&source, b"123456789").unwrap();
        let target = root.join("overlay.zip");
        pack(
            &root,
            &[("vehicles/custom/My build.pc".into(), source)],
            &target,
        )
        .unwrap();
        let entries = super::super::archive_entries(&target).unwrap();
        assert_eq!(entries["vehicles/custom/My build.pc"], (0xcbf43926, 9));
        let bytes = std::fs::read(target).unwrap();
        assert_eq!(&bytes[..4], b"PK\x03\x04");
        assert_eq!(
            u32::from_le_bytes(bytes[14..18].try_into().unwrap()),
            0xcbf43926
        );
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn overlapping_parts_require_restart_when_mount_order_can_change() {
        let m = Manifest {
            catalogue_hash: String::new(),
            stock_hash: String::new(),
            files: vec![
                FileSpec {
                    path: "mods/unpacked/a/vehicles/common/body.jbeam".into(),
                    sha256: "a".repeat(64),
                    byte_length: 1,
                },
                FileSpec {
                    path: "mods/unpacked/b/vehicles/common/body.jbeam".into(),
                    sha256: "b".repeat(64),
                    byte_length: 1,
                },
            ],
        };
        assert!(startup_scripts(Path::new("."), &m, &BTreeMap::new()).unwrap());
        assert!(!startup_scripts(Path::new("."), &m, &m.groups().unwrap()).unwrap());
    }
    #[test]
    fn vehicle_lua_can_reload_but_global_lua_requires_restart() {
        let mut m = Manifest {
            catalogue_hash: String::new(),
            stock_hash: String::new(),
            files: vec![FileSpec {
                path: "mods/unpacked/a/lua/vehicle/custom.lua".into(),
                sha256: "a".repeat(64),
                byte_length: 1,
            }],
        };
        assert!(!startup_scripts(Path::new("."), &m, &BTreeMap::new()).unwrap());
        m.files[0].path = "mods/unpacked/a/lua/ge/extensions/custom.lua".into();
        assert!(startup_scripts(Path::new("."), &m, &BTreeMap::new()).unwrap());
    }
}
