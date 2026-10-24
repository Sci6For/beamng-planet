//! Launch an explicitly configured installation and retain actual process handles.
//! No process is terminated here, including when the companion is dropped.
use crate::protocol::ErrorCode;
use std::{
    error::Error,
    ffi::{OsStr, OsString},
    fmt, io,
    mem::{size_of, zeroed},
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        io::IntoRawHandle,
    },
    path::{Path, PathBuf},
    process::Command,
    ptr,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, ERROR_NO_MORE_FILES, FILETIME, HANDLE,
        INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT,
    },
    Globalization::{CompareStringOrdinal, CSTR_EQUAL},
    Security::{GetLengthSid, GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER},
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
            TH32CS_SNAPPROCESS,
        },
        Threading::{
            CreateMutexW, GetCurrentProcess, GetProcessTimes, OpenProcess, OpenProcessToken,
            QueryFullProcessImageNameW, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION,
            PROCESS_SYNCHRONIZE,
        },
    },
};

const HANDOFF: Duration = Duration::from_secs(30);
const INSPECTION_GRACE: Duration = Duration::from_millis(100);
const LAUNCHER: &str = "BeamNG.drive.exe";
const RENDERER: &str = "Bin64/BeamNG.drive.x64.exe";

#[derive(Debug)]
pub struct GameCommand {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
}
impl GameCommand {
    /// `args` excludes argv[0]. Steam's original command follows the sole `--`.
    pub fn parse(mut args: impl Iterator<Item = OsString>) -> Result<Self, GameError> {
        if args.next().as_deref() != Some(OsStr::new("--")) {
            return Err(GameError::invalid("expected -- before the game command"));
        }
        let executable = args
            .next()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| GameError::invalid("game executable is missing"))?;
        Ok(Self {
            executable: executable.into(),
            arguments: args.collect(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameState {
    LauncherOpen,
    Handoff,
    Running,
    Closed,
    Conflict,
}

#[derive(Debug)]
pub struct GameError {
    code: ErrorCode,
    detail: String,
}
impl GameError {
    pub fn code(&self) -> ErrorCode {
        self.code
    }
    fn invalid(detail: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::InvalidConfig,
            detail: detail.into(),
        }
    }
    fn conflict() -> Self {
        Self {
            code: ErrorCode::GameConflict,
            detail: "a companion or game for this user is already open".into(),
        }
    }
}
impl fmt::Display for GameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.detail.fmt(f)
    }
}
impl Error for GameError {}
impl From<io::Error> for GameError {
    fn from(error: io::Error) -> Self {
        Self::invalid(error.to_string())
    }
}

#[derive(Debug)]
struct Handle(HANDLE);
impl Handle {
    fn checked(raw: HANDLE) -> io::Result<Self> {
        if raw.is_null() || raw == INVALID_HANDLE_VALUE {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self(raw))
        }
    }
    fn alive(&self) -> io::Result<bool> {
        Ok(!self.wait_for_exit(0)?)
    }
    fn wait_for_exit(&self, milliseconds: u32) -> io::Result<bool> {
        // SAFETY: callers use this only on owned process handles with SYNCHRONIZE.
        match unsafe { WaitForSingleObject(self.0, milliseconds) } {
            WAIT_TIMEOUT => Ok(false),
            WAIT_OBJECT_0 => Ok(true),
            _ => Err(io::Error::last_os_error()),
        }
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Identity {
    pid: u32,
    created: u64,
}
#[derive(Debug)]
struct Process {
    handle: Handle,
    id: Identity,
    renderer: bool,
}

#[derive(Debug)]
pub struct GameSession {
    // The named mutex is retained for the entire session, including Closed until
    // drop. It is a presence mutex, never an acquired/abandoned ownership lock.
    _mutex: Handle,
    _baseline: Vec<Process>,
    launcher: Process,
    renderer: Option<Process>,
    paths: [PathBuf; 2],
    user: Vec<u8>,
    handoff_since: Option<Instant>,
    terminal: Option<GameState>,
}

impl GameSession {
    pub fn launch(command: &GameCommand, game_root: &Path) -> Result<Self, GameError> {
        if !game_root.is_absolute() || !command.executable.is_absolute() {
            return Err(GameError::invalid("game paths must be absolute"));
        }
        let root = game_root.canonicalize()?;
        if !root.is_dir() {
            return Err(GameError::invalid("gameRoot must be a directory"));
        }
        let executable = command.executable.canonicalize()?;
        let paths =
            [root.join(LAUNCHER), root.join(RENDERER)].map(|p| p.canonicalize().unwrap_or(p));
        let renderer = same_path(&executable, &paths[1]);
        if !renderer && !same_path(&executable, &paths[0]) {
            return Err(GameError::invalid(
                "command is not the verified launcher or x64 renderer",
            ));
        }
        // SAFETY: GetCurrentProcess is a borrowed pseudohandle, not owned.
        let user = user_sid(unsafe { GetCurrentProcess() })?;
        let mutex = user_mutex(&user)?;
        let baseline = snapshot(&paths, &user, None)?;
        if baseline.iter().any(|p| p.handle.alive().unwrap_or(true)) {
            return Err(GameError::conflict());
        }
        // std::process::Command quotes argv using the Windows CRT rules. No shell,
        // current_dir, env modifications, or CREATE_NO_WINDOW affect the game.
        let child = Command::new(executable).args(&command.arguments).spawn()?;
        let pid = child.id();
        let handle = Handle::checked(child.into_raw_handle())?;
        let created = creation_time(&handle)?;
        let launcher = Process {
            handle,
            id: Identity { pid, created },
            renderer,
        };
        Ok(Self {
            _mutex: mutex,
            _baseline: baseline,
            launcher,
            renderer: None,
            paths,
            user,
            handoff_since: None,
            terminal: None,
        })
    }

    pub fn poll(&mut self) -> Result<GameState, GameError> {
        if let Some(state) = self.terminal {
            return Ok(state);
        }
        if self.launcher.renderer {
            return self.confirm_running(self.launcher.handle.alive()?);
        }
        if let Some(process) = &self.renderer {
            return self.confirm_running(process.handle.alive()?);
        }
        // Snapshot before deciding whether the launcher ended or handoff expired.
        // This is also the final recheck at the 30-second boundary.
        let mut candidates = snapshot(&self.paths, &self.user, Some(&self.launcher))?
            .into_iter()
            .filter(|p| {
                p.renderer
                    && eligible(p.id, self.launcher.id.created, &self._baseline)
                    && p.handle.alive().unwrap_or(false)
            });
        let candidate = candidates.next();
        if candidates.next().is_some() {
            self.terminal = Some(GameState::Conflict);
            return Ok(GameState::Conflict);
        }
        if let Some(process) = candidate {
            self.renderer = Some(process);
            return Ok(GameState::Running);
        }
        if self.launcher.handle.alive()? {
            return Ok(GameState::LauncherOpen);
        }
        let elapsed = self
            .handoff_since
            .get_or_insert_with(Instant::now)
            .elapsed();
        if elapsed < HANDOFF {
            return Ok(GameState::Handoff);
        }
        self.terminal = Some(GameState::Closed);
        Ok(GameState::Closed)
    }

    fn confirm_running(&mut self, alive: bool) -> Result<GameState, GameError> {
        if alive {
            Ok(GameState::Running)
        } else {
            self.terminal = Some(GameState::Closed);
            Ok(GameState::Closed)
        }
    }
}

fn eligible(id: Identity, launched_at: u64, baseline: &[Process]) -> bool {
    id.created >= launched_at && !baseline.iter().any(|p| p.id == id)
}

fn same_path(a: &Path, b: &Path) -> bool {
    let a: Vec<_> = a.as_os_str().encode_wide().collect();
    let b: Vec<_> = b.as_os_str().encode_wide().collect();
    // SAFETY: pointer lengths describe the complete valid UTF-16 buffers.
    unsafe {
        CompareStringOrdinal(a.as_ptr(), a.len() as i32, b.as_ptr(), b.len() as i32, 1)
            == CSTR_EQUAL
    }
}

fn creation_time(handle: &Handle) -> io::Result<u64> {
    let mut created: FILETIME = unsafe { zeroed() };
    let mut exit = created;
    let mut kernel = created;
    let mut user = created;
    if unsafe { GetProcessTimes(handle.0, &mut created, &mut exit, &mut kernel, &mut user) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
}

fn user_sid(process: HANDLE) -> io::Result<Vec<u8>> {
    let mut raw = ptr::null_mut();
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut raw) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let token = Handle::checked(raw)?;
    let mut required = 0;
    unsafe {
        GetTokenInformation(token.0, TokenUser, ptr::null_mut(), 0, &mut required);
    }
    if required == 0 {
        return Err(io::Error::last_os_error());
    }
    // usize storage guarantees TOKEN_USER alignment, unlike Vec<u8>.
    let mut storage = vec![0usize; (required as usize).div_ceil(size_of::<usize>())];
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            storage.as_mut_ptr().cast(),
            required,
            &mut required,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let sid = unsafe { (*(storage.as_ptr().cast::<TOKEN_USER>())).User.Sid };
    let len = unsafe { GetLengthSid(sid) } as usize;
    Ok(unsafe { std::slice::from_raw_parts(sid.cast::<u8>(), len) }.to_vec())
}

fn user_mutex(sid: &[u8]) -> Result<Handle, GameError> {
    let sid_hex: String = sid.iter().map(|b| format!("{b:02x}")).collect();
    // Global namespace gives this user one session across Windows logon sessions.
    let name: Vec<_> = OsStr::new(&format!("Global\\BeamNGEarthCompanion-{sid_hex}"))
        .encode_wide()
        .chain(Some(0))
        .collect();
    let raw = unsafe { CreateMutexW(ptr::null(), 0, name.as_ptr()) };
    let last_error = unsafe { GetLastError() };
    let handle = Handle::checked(raw)?;
    if last_error == ERROR_ALREADY_EXISTS {
        return Err(GameError::conflict());
    }
    Ok(handle)
}

fn snapshot(
    paths: &[PathBuf; 2],
    user: &[u8],
    retained_launcher: Option<&Process>,
) -> io::Result<Vec<Process>> {
    let snapshot = Handle::checked(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) })?;
    let mut entry: PROCESSENTRY32W = unsafe { zeroed() };
    entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
    let mut more = unsafe { Process32FirstW(snapshot.0, &mut entry) };
    let mut result = Vec::new();
    while more != 0 {
        let len = entry
            .szExeFile
            .iter()
            .position(|u| *u == 0)
            .unwrap_or(entry.szExeFile.len());
        let name = PathBuf::from(OsString::from_wide(&entry.szExeFile[..len]));
        // This launcher was verified and opened by launch(). Retaining its
        // handle prevents PID reuse, even after exit; never reopen/requery it
        // while the loader is tearing down its image and access may be denied.
        if !retained_launcher.is_some_and(|p| p.id.pid == entry.th32ProcessID)
            && paths.iter().any(|p| {
                p.file_name()
                    .is_some_and(|n| same_path(Path::new(n), &name))
            })
        {
            if let Ok(handle) = Handle::checked(unsafe {
                OpenProcess(
                    PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                    0,
                    entry.th32ProcessID,
                )
            }) {
                match inspect_with_grace(&handle, || inspect_process(&handle, paths, user)) {
                    Ok(Some((created, renderer))) => result.push(Process {
                        handle,
                        id: Identity {
                            pid: entry.th32ProcessID,
                            created,
                        },
                        renderer,
                    }),
                    Ok(None) => (),
                    Err(error) => {
                        if handle.alive()? {
                            return Err(inspection_error(entry.th32ProcessID, error));
                        }
                    }
                }
            } else {
                let error = io::Error::last_os_error();
                // ERROR_INVALID_PARAMETER means the process disappeared between
                // the snapshot and OpenProcess. Access denial is uncertainty.
                if error.raw_os_error() != Some(87) {
                    return Err(inspection_error(entry.th32ProcessID, error));
                }
            }
        }
        more = unsafe { Process32NextW(snapshot.0, &mut entry) };
    }
    if unsafe { GetLastError() } != ERROR_NO_MORE_FILES {
        return Err(io::Error::last_os_error());
    }
    Ok(result)
}

/// Image queries can fail during startup or teardown before the process handle
/// signals. Retry briefly using the same held handle; only a signaled handle is
/// proof of exit. Persistent uncertainty on a live process remains an error.
fn inspect_with_grace<T>(
    handle: &Handle,
    mut query: impl FnMut() -> io::Result<Option<T>>,
) -> io::Result<Option<T>> {
    let started = Instant::now();
    loop {
        if !handle.alive()? {
            return Ok(None);
        }
        match query() {
            Ok(value) => return Ok(value),
            Err(error) => {
                let remaining = INSPECTION_GRACE.saturating_sub(started.elapsed());
                if remaining.is_zero() {
                    return Err(error);
                }
                if handle.wait_for_exit(remaining.as_millis().clamp(1, 10) as u32)? {
                    return Ok(None);
                }
            }
        }
    }
}

fn inspection_error(pid: u32, error: io::Error) -> io::Error {
    io::Error::new(
        error.kind(),
        format!("cannot inspect potentially matching game process {pid}: {error}"),
    )
}

fn inspect_process(
    handle: &Handle,
    paths: &[PathBuf; 2],
    user: &[u8],
) -> io::Result<Option<(u64, bool)>> {
    if !handle.alive()? {
        return Ok(None);
    }
    let mut buffer = vec![0u16; 32768];
    let mut size = buffer.len() as u32;
    if unsafe { QueryFullProcessImageNameW(handle.0, 0, buffer.as_mut_ptr(), &mut size) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let path = PathBuf::from(OsString::from_wide(&buffer[..size as usize])).canonicalize()?;
    if !paths.iter().any(|p| same_path(p, &path)) || user_sid(handle.0)? != user {
        return Ok(None);
    }
    Ok(Some((creation_time(handle)?, same_path(&path, &paths[1]))))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn current_process_handle() -> Handle {
        Handle::checked(unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                0,
                std::process::id(),
            )
        })
        .unwrap()
    }

    #[test]
    fn transient_query_denial_recovers_while_process_remains_live() {
        let handle = current_process_handle();
        let mut first = true;
        let value = inspect_with_grace(&handle, || {
            if first {
                first = false;
                Err(io::Error::from_raw_os_error(5))
            } else {
                Ok(Some("confirmed renderer"))
            }
        })
        .unwrap();
        assert_eq!(value, Some("confirmed renderer"));
    }

    #[test]
    fn persistent_query_denial_on_live_process_remains_an_error_and_is_bounded() {
        let handle = current_process_handle();
        let started = Instant::now();
        let result = inspect_with_grace::<()>(&handle, || Err(io::Error::from_raw_os_error(5)));
        assert_eq!(result.unwrap_err().raw_os_error(), Some(5));
        assert!(started.elapsed() >= Duration::from_millis(80));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn dying_process_query_denial_becomes_absence_only_after_handle_signals() {
        let child = Command::new(std::env::current_exe().unwrap())
            .arg("--list")
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let handle = Handle::checked(child.into_raw_handle()).unwrap();
        let result =
            inspect_with_grace::<()>(&handle, || Err(io::Error::from_raw_os_error(5))).unwrap();
        assert_eq!(result, None);
        assert!(!handle.alive().unwrap());
    }

    #[test]
    fn prelaunch_and_reused_pid_are_distinguished_by_creation_time() {
        assert!(!eligible(
            Identity {
                pid: 9,
                created: 99
            },
            100,
            &[]
        ));
        assert!(eligible(
            Identity {
                pid: 9,
                created: 101
            },
            100,
            &[]
        ));
        assert_ne!(
            Identity {
                pid: 9,
                created: 99
            },
            Identity {
                pid: 9,
                created: 101
            }
        );
    }
}
