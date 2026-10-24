//! Incremental disk staging; filenames are hashes, never remote filesystem paths.
use anyhow::{bail, Result};
use kissmp_server::world_resources::*;
use shared::planet::*;
use std::{
    collections::{HashMap, HashSet},
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};
struct Pending {
    manifest: ArtifactManifest,
    file: File,
    offset: u64,
    hash: ring::digest::Context,
}
pub struct ArtifactStore {
    root: PathBuf,
    room: Id,
    pending: HashMap<String, Pending>,
}
fn safe_chain(root: &Path) -> Result<()> {
    for p in root.ancestors() {
        if let Ok(m) = std::fs::symlink_metadata(p) {
            if m.file_type().is_symlink() {
                bail!("resource link escape")
            }
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if m.file_attributes() & 0x400 != 0 {
                    bail!("resource reparse escape")
                }
            }
        }
    }
    Ok(())
}
impl ArtifactStore {
    pub fn new(profile: &Path, room: Id) -> Result<Self> {
        safe_chain(profile)?;
        let root = profile.join("temp/beamngEarth/planet").join(id_hex(room));
        safe_chain(&root)?;
        std::fs::create_dir_all(&root)?;
        safe_chain(&root)?;
        Ok(Self {
            root,
            room,
            pending: HashMap::new(),
        })
    }
    pub fn path(&self, r: &ArtifactManifest) -> Result<PathBuf> {
        validate_manifest(r)?;
        safe_chain(&self.root)?;
        let p = self.root.join(&r.logical_key);
        safe_chain(&p)?;
        Ok(p)
    }
    pub fn has(&self, r: &ArtifactManifest) -> Result<bool> {
        let p = self.path(r)?;
        if !p.is_file() {
            return Ok(false);
        };
        let mut f = File::open(p)?;
        if f.metadata()?.len() != r.byte_length.0 {
            return Ok(false);
        };
        let mut h = ring::digest::Context::new(&ring::digest::SHA256);
        let mut b = vec![0; CHUNK];
        loop {
            let n = f.read(&mut b)?;
            if n == 0 {
                break;
            }
            h.update(&b[..n]);
        }
        let sha: String = h
            .finish()
            .as_ref()
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect();
        Ok(sha == r.sha256)
    }
    pub fn stage(&mut self, r: &ArtifactManifest) -> Result<()> {
        validate_manifest(r)?;
        if self.pending.len() >= 2 || self.pending.contains_key(&r.sha256) {
            bail!("active transfer bound")
        };
        let p = self.root.join(format!("{}.part", r.sha256));
        safe_chain(&p)?;
        if p.exists() {
            std::fs::remove_file(&p)?
        };
        let file = OpenOptions::new().write(true).create_new(true).open(p)?;
        self.pending.insert(
            r.sha256.clone(),
            Pending {
                manifest: r.clone(),
                file,
                offset: 0,
                hash: ring::digest::Context::new(&ring::digest::SHA256),
            },
        );
        Ok(())
    }
    pub fn accept_chunk(&mut self, sha: &str, offset: u64, b: &[u8]) -> Result<()> {
        let p = self
            .pending
            .get_mut(sha)
            .ok_or_else(|| anyhow::anyhow!("unknown hash"))?;
        if b.len() > CHUNK
            || offset != p.offset
            || offset
                .checked_add(b.len() as u64)
                .map_or(true, |n| n > p.manifest.byte_length.0)
        {
            bail!("artifact offset/length")
        };
        p.file.write_all(b)?;
        p.hash.update(b);
        p.offset += b.len() as u64;
        Ok(())
    }
    pub fn publish(&mut self, sha: &str) -> Result<PathBuf> {
        let p = self
            .pending
            .remove(sha)
            .ok_or_else(|| anyhow::anyhow!("unknown hash"))?;
        let part = self.root.join(format!("{}.part", sha));
        let actual: String = p
            .hash
            .finish()
            .as_ref()
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect();
        if p.offset != p.manifest.byte_length.0 || actual != sha {
            drop(p.file);
            let _ = std::fs::remove_file(part);
            bail!("truncated/corrupt artifact")
        };
        p.file.sync_all()?;
        drop(p.file);
        let dest = self.path(&p.manifest)?;
        if dest.exists() {
            if !self.has(&p.manifest)? {
                bail!("existing artifact mismatch")
            };
            std::fs::remove_file(part)?
        } else {
            std::fs::rename(part, &dest)?
        };
        Ok(dest)
    }
    pub fn ready(&self, e: &WorldEnvelope) -> Result<serde_json::Value> {
        if e.room != self.room || e.resources.len() > MAX_RESOURCES {
            bail!("envelope identity")
        };
        let hashes: HashSet<_> = e.resources.iter().map(|r| r.sha256.as_str()).collect();
        let mut paths = HashSet::new();
        for r in &e.resources {
            if r.world_epoch != e.world_epoch
                || r.frame_epoch != e.frame_epoch
                || r.dependencies.iter().any(|d| !hashes.contains(d.as_str()))
                || !self.has(r)?
            {
                bail!("unverified dependencies")
            };
            paths.insert(format!(
                "/temp/beamngEarth/planet/{}/{}",
                id_hex(self.room),
                r.logical_key
            ));
        }
        if !EARTH_KINDS.contains(&e.payload.get("type").and_then(|x| x.as_str()).unwrap_or("")) {
            bail!("unknown physical payload")
        };
        validate_data_json(&e.payload, &paths, 0)?;
        for r in &e.resources {
            if r.logical_key.ends_with(".json") {
                if r.byte_length.0 > JSON_ARTIFACT_MAX {
                    bail!("JSON artifact parse bound")
                };
                let v: serde_json::Value = serde_json::from_reader(File::open(self.path(r)?)?)?;
                validate_data_json(&v, &paths, 0)?;
            } else if r.logical_key.ends_with(".cdae") {
                validate_cdae(
                    &mut std::io::BufReader::new(File::open(self.path(r)?)?),
                    &paths,
                )?;
            } else if r.logical_key.ends_with(".dae") {
                if r.byte_length.0 > JSON_ARTIFACT_MAX {
                    bail!("Collada parse bound")
                };
                let text = std::fs::read_to_string(self.path(r)?)?;
                if text.contains("<!ENTITY") || text.contains("<!DOCTYPE") {
                    bail!("Collada external entity")
                };
                for segment in text.split('<').skip(1) {
                    let tag = segment.split('>').next().unwrap_or("");
                    if tag
                        .split_whitespace()
                        .next()
                        .unwrap_or("")
                        .ends_with("init_from")
                    {
                        let reference = segment.split('>').nth(1).unwrap_or("").trim();
                        if !reference.starts_with('/') && !reference.starts_with('#') {
                            bail!("relative Collada resource")
                        };
                        validate_data_string(reference, &paths)?;
                    }
                }
            }
        }
        Ok(e.payload.clone())
    }
}
impl Drop for ArtifactStore {
    fn drop(&mut self) {
        for (sha, p) in self.pending.drain() {
            drop(p.file);
            let _ = std::fs::remove_file(self.root.join(format!("{}.part", sha)));
        }
    }
}
