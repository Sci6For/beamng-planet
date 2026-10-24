use serde::{Deserialize, Serialize};
use std::{
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
};

pub const OFFICIAL_BACKEND_SHA256: &str =
    "f1187d7decaf37e3b7a62b637921c972bd311def3ee4229d509f67024ec2dabd";
/// A LAN candidate pins the backend image generated in the same build. Legacy
/// planet6 builds retain their reviewed hash when no build pin is supplied.
pub const APPROVED_BACKEND_SHA256: &str =
    match option_env!("PLANET_BACKEND_SHA256") {
        Some(pin) => pin,
        None => OFFICIAL_BACKEND_SHA256,
    };

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    pub game_root: PathBuf,
    pub profile: PathBuf,
    pub cache_dir: PathBuf,
    pub backend_exe: PathBuf,
    pub backend_sha256: String,
}

#[derive(Debug)]
pub struct ConfigError(String);

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl Error for ConfigError {}

impl Config {
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        if !path.is_absolute() {
            return Err(ConfigError("configuration path must be absolute".into()));
        }
        let bytes =
            fs::read(path).map_err(|e| ConfigError(format!("cannot read configuration: {e}")))?;
        let config: Self = serde_json::from_slice(&bytes)
            .map_err(|e| ConfigError(format!("invalid configuration: {e}")))?;
        config.validate()
    }

    /// Resolve only the explicitly configured paths. Missing profiles are errors;
    /// validation never discovers or silently selects an alternate profile.
    /// Backend content hashing is performed by the backend module before launch.
    pub fn validate(mut self) -> Result<Self, ConfigError> {
        self.profile = checked_path(&self.profile, "profile", true)?;
        self.game_root = checked_path(&self.game_root, "gameRoot", true)?;
        self.cache_dir = checked_path(&self.cache_dir, "cacheDir", true)?;
        self.backend_exe = checked_path(&self.backend_exe, "backendExe", false)?;
        if self.backend_sha256 != APPROVED_BACKEND_SHA256 {
            return Err(ConfigError(
                "backendSha256 does not match the backend pinned into this helper".into(),
            ));
        }
        Ok(self)
    }

    /// Immutable package-owned pins; never supplied by game or peer intent.
    pub fn lan_inventory_path(&self) -> PathBuf {
        self.backend_exe.parent().unwrap().join("lan-images.json")
    }
    pub fn lan_runtime_root(&self) -> PathBuf {
        self.profile.join("settings/beamngEarthAutostart/lan")
    }

    pub fn request_path(&self) -> PathBuf {
        self.profile
            .join("settings/beamngEarthAutostart/request.json")
    }

    pub fn status_path(&self) -> PathBuf {
        self.profile
            .join("settings/beamngEarthAutostart/status.json")
    }
}

fn checked_path(path: &Path, field: &str, directory: bool) -> Result<PathBuf, ConfigError> {
    if !path.is_absolute() {
        return Err(ConfigError(format!("{field} must be absolute")));
    }
    let resolved = path
        .canonicalize()
        .map_err(|e| ConfigError(format!("{field} is unavailable: {e}")))?;
    if if directory {
        !resolved.is_dir()
    } else {
        !resolved.is_file()
    } {
        return Err(ConfigError(format!("{field} has the wrong path type")));
    }
    Ok(resolved)
}
