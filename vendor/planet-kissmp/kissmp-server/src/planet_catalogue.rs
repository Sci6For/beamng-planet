use super::*;
use std::convert::TryInto;
use std::io::{Read, Seek, SeekFrom};
/// Read stock ZIP central directories without extracting or trusting executable paths.
/// ZIP64 directory offsets are required by the measured >4 GiB common stock archive.
pub fn stock_catalogue(game_root: &Path) -> Result<(BTreeSet<String>, String)> {
    let mut entries = std::collections::BTreeMap::<String, (u32, u32)>::new();
    for path in std::fs::read_dir(game_root.join("content/vehicles"))? {
        let path = path?.path();
        if path.extension().and_then(|s| s.to_str()) != Some("zip") {
            continue;
        }
        for (name, fingerprint) in archive_entries(&path)? {
            if name.ends_with(".pc") && entries.insert(name, fingerprint).is_some() {
                bail!("duplicate stock config")
            }
        }
    }
    if entries.is_empty() {
        bail!("missing stock catalogue")
    }
    let digest = descriptor_bytes_digest(&serde_json::to_vec(&entries)?);
    Ok((entries.into_keys().collect(), digest))
}

fn archive_entries(path: &Path) -> Result<std::collections::BTreeMap<String, (u32, u32)>> {
        let mut entries = std::collections::BTreeMap::new();
        let mut file = std::fs::File::open(path)?;
        let length = file.metadata()?.len();
        let tail_len = length.min(65557) as usize;
        file.seek(SeekFrom::End(-(tail_len as i64)))?;
        let mut tail = vec![0; tail_len];
        file.read_exact(&mut tail)?;
        let end = (0..tail.len().saturating_sub(21))
            .rev()
            .find(|i| tail[*i..].starts_with(b"PK\x05\x06"))
            .ok_or_else(|| anyhow::anyhow!("ZIP directory"))?;
        let u16at = |o| u16::from_le_bytes([tail[end + o], tail[end + o + 1]]);
        let u32at = |o| u32::from_le_bytes(tail[end + o..end + o + 4].try_into().unwrap());
        let mut count = u16at(10) as u64;
        let mut size = u32at(12) as u64;
        let mut offset = u32at(16) as u64;
        if count == 65535 || size == u32::MAX as u64 || offset == u32::MAX as u64 {
            if end < 20 || &tail[end - 20..end - 16] != b"PK\x06\x07" {
                bail!("missing ZIP64 locator")
            }
            let zip64_offset = u64::from_le_bytes(tail[end - 12..end - 4].try_into().unwrap());
            file.seek(SeekFrom::Start(zip64_offset))?;
            let mut record = [0; 56];
            file.read_exact(&mut record)?;
            if &record[..4] != b"PK\x06\x06" || record[16..24] != [0; 8] {
                bail!("invalid ZIP64 directory")
            }
            count = u64::from_le_bytes(record[32..40].try_into().unwrap());
            size = u64::from_le_bytes(record[40..48].try_into().unwrap());
            offset = u64::from_le_bytes(record[48..56].try_into().unwrap());
        }
        if count > 100000
            || size > 16 * 1024 * 1024
            || offset.checked_add(size).map_or(true, |end| end > length)
            || u16at(4) != 0
            || u16at(6) != 0
        {
            bail!("unsupported ZIP directory")
        }
        file.seek(SeekFrom::Start(offset))?;
        for _ in 0..count {
            if file.stream_position()? + 46 > offset + size {
                bail!("ZIP directory bounds")
            }
            let mut header = [0u8; 46];
            file.read_exact(&mut header)?;
            if &header[..4] != b"PK\x01\x02" {
                bail!("ZIP entry")
            }
            let get16 = |o| u16::from_le_bytes([header[o], header[o + 1]]);
            let get32 = |o| u32::from_le_bytes(header[o..o + 4].try_into().unwrap());
            let name_len = get16(28) as usize;
            if name_len == 0 || name_len > 1024 {
                bail!("ZIP name")
            }
            if file.stream_position()? + name_len as u64 + get16(30) as u64 + get16(32) as u64
                > offset + size
            {
                bail!("ZIP directory bounds")
            }
            let mut name = vec![0; name_len];
            file.read_exact(&mut name)?;
            let name = String::from_utf8(name)?;
            file.seek(SeekFrom::Current((get16(30) as i64) + (get16(32) as i64)))?;
            let vehicle = name.starts_with("vehicles/");
            let dependency = ["lua/", "art/", "sound/"].iter().any(|prefix| name.starts_with(prefix));
            if !vehicle && !dependency && !name.ends_with(".pc") {
                continue;
            }
            if name.ends_with('/') { continue; }
            let parts: Vec<_> = name.split('/').collect();
            if parts.len() < 2
                || (!vehicle && !dependency)
                || parts.iter().any(|s| {
                    s.is_empty() || *s == "." || *s == ".." || s.contains('\\') || s.contains(':')
                })
                || get16(8) & 1 != 0
                || (name.ends_with(".pc") && (!vehicle || parts.len() != 3 || get32(24) as usize > CONTROL_MAX / 2))
            {
                bail!("unsafe vehicle archive entry")
            }
            if entries.insert(name, (get32(16), get32(24))).is_some() {
                bail!("duplicate vehicle archive entry")
            }
        }
    Ok(entries)
}

/// Match the current profile's enabled vehicle resources. The catalogue stays a
/// strict room contract: identical installed mods/configs on both machines, no
/// downloading or fallback to a different car. Whole mod bytes cover JBeam,
/// materials and scripts as well as the selectable .pc files.
pub fn installed_catalogue(game_root: &Path, profile_root: &Path) -> Result<(BTreeSet<String>, String)> {
    let (mut members, stock_digest) = stock_catalogue(game_root)?;
    let mods = profile_root.join("mods");
    let mut disabled = BTreeSet::new();
    let database = mods.join("db.json");
    if database.is_file() {
        let bytes = std::fs::read(database)?;
        if bytes.len() > 16 * 1024 * 1024 { bail!("mod database too large"); }
        let db: serde_json::Value = serde_json::from_slice(&bytes)?;
        let records = db["mods"].as_object().ok_or_else(||anyhow::anyhow!("invalid mod database"))?;
        for record in records.values() {
            if record["active"].as_bool() == Some(false) {
                if let Some(path) = record["fullpath"].as_str() {
                    disabled.insert(path.trim_matches('/').to_lowercase());
                }
            }
        }
    }
    let mut fingerprints = std::collections::BTreeMap::new();
    if mods.is_dir() {
        for path in files(&mods)? {
            if path.extension().and_then(|s|s.to_str()).map_or(true,|s|!s.eq_ignore_ascii_case("zip"))
                || path.starts_with(mods.join("unpacked")) { continue; }
            let relative=relative(profile_root,&path)?;
            if disabled.contains(&relative.to_lowercase()) { continue; }
            let entries=archive_entries(&path)?;
            if entries.is_empty() { continue; }
            add_configs(&mut members,entries.keys().map(String::as_str))?;
            fingerprints.insert(relative,crate::world_authority::hash_file(&path)?);
        }
        let unpacked=mods.join("unpacked");
        if unpacked.is_dir() {
            for entry in std::fs::read_dir(&unpacked)? {
                let path=entry?.path(); reject_link(&path)?;
                if !path.is_dir() || disabled.contains(&relative(profile_root,&path)?.to_lowercase()) || !["vehicles", "lua", "art", "sound"].iter().any(|name|path.join(name).is_dir()) { continue; }
                let mut manifest=std::collections::BTreeMap::new();
                for file in files(&path)? {
                    let name=relative(&path,&file)?;
                    if name.starts_with("vehicles/") && name.ends_with(".pc") { add_configs(&mut members,[name.as_str()])?; }
                    manifest.insert(name,crate::world_authority::hash_file(&file)?);
                }
                fingerprints.insert(relative(profile_root,&path)?,descriptor_bytes_digest(&serde_json::to_vec(&manifest)?));
            }
        }
    }
    // Configurations saved by the player live outside the mods directory.
    let saved=profile_root.join("vehicles");
    if saved.is_dir() {
        let mut manifest=std::collections::BTreeMap::new();
        for path in files(&saved)? {
            let name=relative(profile_root,&path)?;
            if !name.ends_with(".pc") { continue; }
            add_configs(&mut members,[name.as_str()])?;
            manifest.insert(name,crate::world_authority::hash_file(&path)?);
        }
        if !manifest.is_empty() { fingerprints.insert("vehicles".to_string(),descriptor_bytes_digest(&serde_json::to_vec(&manifest)?)); }
    }
    if fingerprints.is_empty() { return Ok((members,stock_digest)); }
    let digest=descriptor_bytes_digest(&serde_json::to_vec(&(stock_digest,fingerprints))?);
    Ok((members,digest))
}
fn add_configs<'a>(members:&mut BTreeSet<String>, names:impl IntoIterator<Item=&'a str>)->Result<()> {
    for name in names {
        if !name.ends_with(".pc") { continue; }
        let parts:Vec<_>=name.split('/').collect();
        if parts.len()!=3 || parts[0]!="vehicles" || parts.iter().any(|s|s.is_empty() || *s=="." || *s==".." || s.contains(['\\',':'])) { bail!("unsafe vehicle config"); }
        if !members.insert(name.into()) { bail!("conflicting vehicle config: {}",name); }
    }
    Ok(())
}
fn relative(root:&Path,path:&Path)->Result<String> { Ok(path.strip_prefix(root)?.to_str().ok_or_else(||anyhow::anyhow!("invalid mod filename"))?.replace('\\',"/")) }
fn reject_link(path:&Path)->Result<()> {
    let metadata=std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() { bail!("vehicle mod link"); }
    #[cfg(windows)] { use std::os::windows::fs::MetadataExt; if metadata.file_attributes() & 0x400 != 0 { bail!("vehicle mod reparse point"); } }
    Ok(())
}
fn files(root:&Path)->Result<Vec<std::path::PathBuf>> {
    reject_link(root)?;
    let mut out=Vec::new(); let mut pending=vec![root.to_path_buf()];
    while let Some(dir)=pending.pop() {
        for entry in std::fs::read_dir(dir)? {
            let path=entry?.path(); reject_link(&path)?;
            if path.is_dir() { pending.push(path); } else { out.push(path); }
            if out.len()+pending.len()>200000 { bail!("vehicle mod file limit"); }
        }
    }
    out.sort(); Ok(out)
}
