//! Official backend arguments and bounded protocol-4 readiness exchange.
use crate::{config::Config, protocol::ErrorCode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    fmt,
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    os::windows::fs::OpenOptionsExt,
    path::Path,
    time::Instant,
};

#[derive(Debug)]
pub struct BackendError {
    code: ErrorCode,
    detail: String,
}
impl BackendError {
    pub(crate) fn new(code: ErrorCode, detail: impl ToString) -> Self {
        Self {
            code,
            detail: detail.to_string(),
        }
    }
    pub fn code(&self) -> ErrorCode {
        self.code
    }
}
impl fmt::Display for BackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.detail)
    }
}
impl std::error::Error for BackendError {}
impl From<io::Error> for BackendError {
    fn from(e: io::Error) -> Self {
        Self::new(ErrorCode::BackendStartFailed, e)
    }
}
pub type Result<T> = std::result::Result<T, BackendError>;
pub const ADDRESS: &str = "127.0.0.1:47613";
pub const MAX_FRAME: usize = 16 * 1024 * 1024;

pub fn arguments(config: &Config) -> Vec<OsString> {
    let mut a: Vec<OsString> = ["--server", "--port", "47613", "--user-dir"]
        .into_iter()
        .map(Into::into)
        .collect();
    a.push(config.profile.clone().into());
    a.push("--cache-dir".into());
    a.push(config.cache_dir.clone().into());
    a.extend(
        [
            "--deshadow",
            "full",
            "--near-visual",
            "detail",
            "--airports",
            "on",
            "--sea",
            "on",
            "--trees",
            "on",
            "--buildings",
            "on",
        ]
        .into_iter()
        .map(OsString::from),
    );
    a
}
/// Preflight only; readiness still checks listener ownership after launch.
pub fn ensure_port_available(address: SocketAddr) -> Result<()> {
    if !address.ip().is_loopback() || !address.is_ipv4() {
        return Err(BackendError::new(
            ErrorCode::InvalidConfig,
            "backend must bind IPv4 loopback",
        ));
    }
    TcpListener::bind(address)
        .map(drop)
        .map_err(|e| BackendError::new(ErrorCode::PortBusy, e))
}
/// Hold this read-only, non-write/non-delete-shared handle through CreateProcessW.
/// This prevents executable replacement between hashing and process creation.
pub(crate) fn verified_file(path: &Path, expected: &str) -> Result<File> {
    if !path.is_absolute() {
        return Err(BackendError::new(
            ErrorCode::InvalidConfig,
            "backend path must be absolute",
        ));
    }
    let mut file = OpenOptions::new().read(true).share_mode(1).open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    if format!("{:x}", hash.finalize()) != expected {
        return Err(BackendError::new(
            ErrorCode::BackendHashMismatch,
            "backend SHA-256 mismatch",
        ));
    }
    Ok(file)
}
pub fn verify_hash(path: &Path, expected: &str) -> Result<()> {
    verified_file(path, expected).map(drop)
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Hello {
    #[serde(rename = "type")]
    pub kind: String,
    pub protocol: u32,
    pub format: u32,
    pub server_version: String,
    pub session_id: String,
    pub far_format: String,
    pub material: String,
    pub near_visual: String,
    pub attribution: String,
    pub cells: u32,
}
fn remaining(deadline: Instant) -> Result<std::time::Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
        .ok_or_else(|| BackendError::new(ErrorCode::BackendReadyTimeout, "hello deadline expired"))
}
fn read_exact(stream: &mut TcpStream, mut bytes: &mut [u8], deadline: Instant) -> Result<()> {
    while !bytes.is_empty() {
        stream.set_read_timeout(Some(remaining(deadline)?))?;
        match stream.read(bytes) {
            Ok(0) => {
                return Err(BackendError::new(
                    ErrorCode::BackendFailed,
                    "EOF in hello frame",
                ))
            }
            Ok(n) => {
                bytes = &mut bytes[n..];
            }
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                ) =>
            {
                return Err(BackendError::new(ErrorCode::BackendReadyTimeout, e))
            }
            Err(e) => return Err(BackendError::new(ErrorCode::BackendFailed, e)),
        }
    }
    Ok(())
}
/// Wire exchange only. Runtime must use OwnedBackend::probe for ownership checks.
pub(crate) fn probe(
    address: SocketAddr,
    deadline: Instant,
    verify: impl Fn(&TcpStream) -> Result<()>,
) -> Result<Hello> {
    let mut stream = TcpStream::connect_timeout(&address, remaining(deadline)?)
        .map_err(|e| BackendError::new(ErrorCode::BackendReadyTimeout, e))?;
    verify(&stream)?;
    stream.set_write_timeout(Some(remaining(deadline)?))?;
    let client = br#"{"type":"hello","protocol":4,"format":4}"#;
    stream.write_all(&(client.len() as u32).to_le_bytes())?;
    stream.write_all(client)?;
    // Official server can emit bounded status messages before hello.
    for _ in 0..32 {
        let mut prefix = [0; 4];
        read_exact(&mut stream, &mut prefix, deadline)?;
        let length = u32::from_le_bytes(prefix) as usize;
        if length == 0 || length > MAX_FRAME {
            return Err(BackendError::new(
                ErrorCode::BackendFailed,
                "invalid hello frame length",
            ));
        }
        let mut payload = vec![0; length];
        read_exact(&mut stream, &mut payload, deadline)?;
        let value: serde_json::Value = serde_json::from_slice(&payload)
            .map_err(|e| BackendError::new(ErrorCode::BackendFailed, e))?;
        if value.get("type").and_then(|v| v.as_str()) == Some("status") {
            continue;
        }
        let hello: Hello = serde_json::from_value(value)
            .map_err(|e| BackendError::new(ErrorCode::BackendFailed, e))?;
        if hello.kind != "hello"
            || hello.protocol != 4
            || hello.format != 4
            || hello.server_version.is_empty()
            || hello.session_id.is_empty()
            || !["dae", "cdae"].contains(&hello.far_format.as_str())
            || hello.material.is_empty()
            || !["mesh", "terrain", "detail"].contains(&hello.near_visual.as_str())
            || ![32, 64].contains(&hello.cells)
        {
            return Err(BackendError::new(
                ErrorCode::BackendFailed,
                "incompatible backend hello",
            ));
        }
        verify(&stream)?;
        remaining(deadline)?;
        return Ok(hello);
    }
    Err(BackendError::new(
        ErrorCode::BackendFailed,
        "too many messages before hello",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn production_arguments_preserve_selected_paths_and_launcher_recommendations() {
        let config = Config {
            game_root: "C:/game".into(),
            profile: "C:/my profile/профиль".into(),
            cache_dir: "D:/selected cache".into(),
            backend_exe: "C:/server.exe".into(),
            backend_sha256: String::new(),
        };
        let args = arguments(&config);
        let expected = [
            "--server",
            "--port",
            "47613",
            "--user-dir",
            "C:/my profile/профиль",
            "--cache-dir",
            "D:/selected cache",
            "--deshadow",
            "full",
            "--near-visual",
            "detail",
            "--airports",
            "on",
            "--sea",
            "on",
            "--trees",
            "on",
            "--buildings",
            "on",
        ];
        assert_eq!(args, expected.map(OsString::from));
    }
    #[test]
    fn sha256_accepts_known_digest_and_rejects_content_changes() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../artifacts/backend-hash");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.canonicalize().unwrap().join("sample");
        std::fs::write(&path, b"abc").unwrap();
        let digest = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        verify_hash(&path, digest).unwrap();
        std::fs::write(&path, b"abcd").unwrap();
        assert_eq!(
            verify_hash(&path, digest).unwrap_err().code(),
            ErrorCode::BackendHashMismatch
        );
    }
}
