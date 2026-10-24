//! Disk IPC and game-bound orchestration. Only the owned backend may be stopped.
use crate::{
    backend::{self, BackendError},
    config::Config,
    game::{GameSession, GameState},
    job::OwnedBackend,
    lifecycle::{Action, Event, Lifecycle},
    protocol::{
        ErrorCode, Observation, Request, RequestState, RequestTracker, Status, StatusState,
        FORMAT_VERSION, MAX_REQUEST_BYTES,
    },
};
use std::{
    collections::VecDeque,
    error::Error,
    fmt,
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::Storage::FileSystem::{
    MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
};

const FILE_POLL: Duration = Duration::from_millis(500);
const PROCESS_POLL: Duration = Duration::from_millis(100);
const PROBE_POLL: Duration = Duration::from_secs(1);
const PROBE_BUDGET: Duration = Duration::from_millis(150);
const FILE_LOCK_BUDGET: Duration = Duration::from_millis(150);
const LOG_LIMIT: u64 = 64 * 1024;

#[derive(Debug)]
pub struct RuntimeError {
    code: ErrorCode,
    detail: String,
}
impl RuntimeError {
    pub fn code(&self) -> ErrorCode {
        self.code
    }
    fn new(code: ErrorCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}
impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.detail)
    }
}
impl Error for RuntimeError {}
fn profile_error(error: impl fmt::Display) -> RuntimeError {
    RuntimeError::new(
        ErrorCode::ProfileUnavailable,
        format!("Профиль игры недоступен: {error}"),
    )
}

fn retry_file_lock<T>(mut operation: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    let deadline = Instant::now() + FILE_LOCK_BUDGET;
    loop {
        match operation() {
            // Windows replacement of a read-locked file can report ACCESS_DENIED
            // rather than SHARING_VIOLATION. Persistent ACL denial stays an error.
            Err(error)
                if matches!(error.raw_os_error(), Some(5 | 32 | 33))
                    && Instant::now() < deadline =>
            {
                thread::sleep(
                    Duration::from_millis(10)
                        .min(deadline.saturating_duration_since(Instant::now())),
                );
            }
            result => return result,
        }
    }
}

/// One validated profile directory. Invalid/partial files never advance sequence.
/// Preview, launching and active need sequence progress after a baseline; a validated idle baseline
/// is terminal intent and must reach lifecycle even when no heartbeat follows.
pub struct Exchange {
    directory: PathBuf,
    tracker: RequestTracker,
    session: String,
    latest: Option<Request>,
    lan_status: Option<crate::lan_protocol::LanStatus>,
}
impl Exchange {
    pub fn open(directory: &Path) -> Result<Self, RuntimeError> {
        fs::create_dir_all(directory).map_err(profile_error)?;
        if !directory.is_dir() {
            return Err(profile_error("каталог настроек отсутствует"));
        }
        Ok(Self {
            directory: directory.into(),
            tracker: RequestTracker::default(),
            session: String::new(),
            latest: None,
            lan_status: None,
        })
    }
    fn check_directory(&self) -> Result<(), RuntimeError> {
        if !self.directory.is_dir() {
            return Err(profile_error("каталог настроек исчез"));
        }
        Ok(())
    }
    pub fn receive(&mut self) -> Result<Option<Event>, RuntimeError> {
        self.check_directory()?;
        let bytes = match retry_file_lock(|| {
            let file = fs::File::open(self.directory.join("request.json"))?;
            let mut bytes = Vec::new();
            file.take((MAX_REQUEST_BYTES + 1) as u64)
                .read_to_end(&mut bytes)?;
            Ok(bytes)
        }) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(profile_error(error)),
        };
        let observation = self.tracker.observe(&bytes);
        if observation != Observation::Ignored {
            // Tracker already validated this exact bounded buffer.
            let request: Request = serde_json::from_slice(&bytes).map_err(profile_error)?;
            self.session = request.session.clone();
            if observation != Observation::Baseline {
                self.latest = Some(request.clone());
            }
            if observation == Observation::Baseline && request.state == RequestState::Idle {
                // A reload can be unloaded before its first disk poll. Idle
                // cannot start a backend, so preserve this final stop signal.
                return Ok(Some(Event::Idle));
            }
        }
        Ok(match observation {
            Observation::Active => Some(Event::Active),
            Observation::Idle => Some(Event::Idle),
            _ => None,
        })
    }
    pub fn publish(
        &mut self,
        state: StatusState,
        error_code: Option<ErrorCode>,
    ) -> Result<(), RuntimeError> {
        self.check_directory()?;
        let status = Status {
            lan: self.lan_status.clone(),
            format_version: FORMAT_VERSION,
            session: self.session.clone(),
            state,
            error_code,
        };
        let bytes = serde_json::to_vec(&status).map_err(profile_error)?;
        let temporary = self.directory.join("status.tmp");
        let target = self.directory.join("status.json");
        let result = retry_file_lock(|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            let wide = |p: &Path| {
                p.as_os_str()
                    .encode_wide()
                    .chain(Some(0))
                    .collect::<Vec<_>>()
            };
            if unsafe {
                MoveFileExW(
                    wide(&temporary).as_ptr(),
                    wide(&target).as_ptr(),
                    MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result.map_err(profile_error)
    }
}

struct RuntimeLog {
    path: PathBuf,
}
impl RuntimeLog {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }
    fn write(&mut self, message: &str) -> Result<(), RuntimeError> {
        // Runtime emits only fixed state/error labels, never request payloads.
        let line = format!(
            "{} {message}\n",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
        );
        if fs::metadata(&self.path)
            .map(|m| m.len() + line.len() as u64 > LOG_LIMIT)
            .unwrap_or(false)
        {
            let backup = self.path.with_file_name("runtime.log.1");
            if backup.exists() {
                fs::remove_file(&backup).map_err(profile_error)?;
            }
            fs::rename(&self.path, backup).map_err(profile_error)?;
        }
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .and_then(|mut f| f.write_all(line.as_bytes()))
            .map_err(profile_error)
    }
}

pub fn run(config: Config, game: GameSession) -> Result<(), RuntimeError> {
    // Production has no fixture mode: validate/pin/hash and retain backend image.
    run_loop(config, game, OwnedBackend::start)
}

fn run_loop(
    mut config: Config,
    mut game: GameSession,
    mut start: impl FnMut(&Config, &Path) -> Result<OwnedBackend, BackendError>,
) -> Result<(), RuntimeError> {
    config = config
        .validate()
        .map_err(|e| RuntimeError::new(ErrorCode::InvalidConfig, e.to_string()))?;
    let directory = config.status_path().parent().unwrap().to_path_buf();
    let mut exchange = Exchange::open(&directory)?;
    let mut log = RuntimeLog::new(directory.join("runtime.log"));
    let backend_log = directory.join("backend.log");
    let mut lifecycle = Lifecycle::default();
    let mut lan = crate::lan_runtime::LanRuntime::default();
    let mut audio = crate::audio::AudioRuntime::default();
    let mut guest_active = false;
    let origin = Instant::now();
    let mut next_file = origin;
    let mut next_probe = origin;
    let mut owned: Option<OwnedBackend> = None;
    let mut state = StatusState::Waiting;
    let mut error = None;
    exchange.publish(state, error)?;
    log.write("Waiting")?;
    loop {
        // The retained game handle is checked before IPC and every probe slice.
        let game_state = game
            .poll()
            .map_err(|e| RuntimeError::new(e.code(), e.to_string()))?;
        let exited = match game_state {
            GameState::Conflict => {
                return Err(RuntimeError::new(
                    ErrorCode::GameConflict,
                    "Обнаружена другая игровая сессия",
                ))
            }
            GameState::Closed => true,
            _ => false,
        };
        let mut events = VecDeque::new();
        if exited {
            events.push_back(Event::GameExited);
        } else {
            let now = Instant::now();
            if now >= next_file {
                next_file = now + FILE_POLL;
                let previous = exchange.session.clone();
                // A launcher-only session cannot authorize a server, even if a
                // previous run left an active request in this profile.
                if game_state == GameState::Running {
                    if let Some(event) = exchange.receive()? {
                        events.push_back(event);
                    }
                    if previous != exchange.session {
                        lan.stop();
                    }
                    if let Some(request) = exchange.latest.take() {
                        if request.state == RequestState::Idle {
                            lan.stop();
                            guest_active = false;
                        } else {
                            lan.apply(
                                &config,
                                request.mode,
                                request.lan.as_ref(),
                                request.sequence,
                            );
                            guest_active = request.mode == crate::lan_protocol::Mode::Join
                                && request.state == RequestState::Active;
                        }
                    }
                    lan.poll();
                    audio.tick(&directory, &exchange.session);
                    if guest_active
                        && lan
                            .status
                            .as_ref()
                            .is_some_and(|s| s.state == crate::lan_protocol::LanState::Active)
                    {
                        events.retain(|e| *e != Event::Active);
                        events.push_back(Event::Idle);
                    }
                    exchange.lan_status = lan.status.clone();
                    exchange.publish(state, error)?;
                    if previous != exchange.session {
                        exchange.publish(state, error)?;
                    }
                }
            }
            if let Some(backend) = owned.as_ref() {
                match backend.is_running() {
                    Ok(false) => events.push_back(Event::BackendFailed(ErrorCode::BackendFailed)),
                    Err(e) => events.push_back(Event::BackendFailed(e.code())),
                    Ok(true) if state == StatusState::Starting && now >= next_probe => {
                        next_probe = now + PROBE_POLL;
                        // Failed hello attempts don't prove process death. The
                        // lifecycle alone owns the overall 45-second deadline.
                        if backend
                            .probe(
                                backend::ADDRESS.parse().unwrap(),
                                Instant::now() + PROBE_BUDGET,
                            )
                            .is_ok()
                        {
                            events.push_back(Event::BackendReady);
                        }
                    }
                    _ => {}
                }
            }
            events.push_back(Event::Tick);
        }
        while let Some(event) = events.pop_front() {
            let mut actions: VecDeque<_> = lifecycle.step(event, origin.elapsed()).into();
            while let Some(action) = actions.pop_front() {
                match action {
                    Action::Publish {
                        state: next,
                        error_code,
                    } => {
                        state = next;
                        error = error_code;
                        exchange.publish(state, error)?;
                        log.write(&format!("{state:?} {error:?}"))?;
                    }
                    Action::StartBackend => match start(&config, &backend_log) {
                        Ok(backend) => {
                            owned = Some(backend);
                            next_probe = Instant::now();
                        }
                        Err(e) => events.push_front(Event::BackendFailed(e.code())),
                    },
                    Action::StopBackend => {
                        if let Some(mut backend) = owned.take() {
                            backend
                                .stop()
                                .map_err(|e| RuntimeError::new(e.code(), e.to_string()))?;
                        }
                    }
                    Action::Exit => return Ok(()),
                }
            }
        }
        thread::sleep(PROCESS_POLL);
    }
}

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod tests;
