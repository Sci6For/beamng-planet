//! Backend-only Job Object. Games are never assigned to or killed by this module.
use crate::{
    backend::{self, BackendError, Hello, Result},
    config::Config,
    protocol::ErrorCode,
};
use std::{
    ffi::OsString,
    fs::{self, File},
    io::{self, Read, Write},
    mem::{size_of, zeroed},
    net::SocketAddr,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::{Path, PathBuf},
    ptr::{null, null_mut},
    thread::{self, JoinHandle},
    time::Instant,
};
use windows_sys::Win32::{
    Foundation::{SetHandleInformation, ERROR_INSUFFICIENT_BUFFER, HANDLE, HANDLE_FLAG_INHERIT},
    NetworkManagement::IpHelper::{
        GetExtendedTcpTable, MIB_TCPROW_OWNER_PID, TCP_TABLE_OWNER_PID_CONNECTIONS,
        TCP_TABLE_OWNER_PID_LISTENER,
    },
    Networking::WinSock::AF_INET,
    Security::SECURITY_ATTRIBUTES,
    Storage::FileSystem::{
        CreateFileW, FILE_GENERIC_READ, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    },
    System::{JobObjects::*, Pipes::CreatePipe, Threading::*},
};

fn os_error() -> BackendError {
    io::Error::last_os_error().into()
}
fn wide(s: &std::ffi::OsStr) -> Result<Vec<u16>> {
    let mut v: Vec<_> = s.encode_wide().collect();
    if v.contains(&0) {
        return Err(BackendError::new(
            ErrorCode::InvalidConfig,
            "NUL in backend path or argument",
        ));
    }
    v.push(0);
    Ok(v)
}
fn quoted(arg: &std::ffi::OsStr, out: &mut Vec<u16>) -> Result<()> {
    let raw = wide(arg)?;
    out.push(34);
    let mut slashes = 0;
    for &c in &raw[..raw.len() - 1] {
        if c == 92 {
            slashes += 1;
            continue;
        }
        if c == 34 {
            out.extend(std::iter::repeat(92).take(slashes * 2 + 1));
        } else {
            out.extend(std::iter::repeat(92).take(slashes));
        }
        slashes = 0;
        out.push(c);
    }
    out.extend(std::iter::repeat(92).take(slashes * 2));
    out.push(34);
    Ok(())
}
unsafe fn own(handle: HANDLE) -> OwnedHandle {
    OwnedHandle::from_raw_handle(handle)
}
struct Attributes {
    storage: Vec<usize>,
    initialized: bool,
}
impl Attributes {
    fn new(handles: &[HANDLE]) -> Result<Self> {
        unsafe {
            let mut bytes = 0;
            InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut bytes);
            let mut this = Self {
                storage: vec![0; bytes.div_ceil(size_of::<usize>())],
                initialized: false,
            };
            if InitializeProcThreadAttributeList(this.ptr(), 1, 0, &mut bytes) == 0 {
                return Err(os_error());
            }
            this.initialized = true;
            if UpdateProcThreadAttribute(
                this.ptr(),
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                handles.as_ptr().cast(),
                std::mem::size_of_val(handles),
                null_mut(),
                null(),
            ) == 0
            {
                return Err(os_error());
            }
            Ok(this)
        }
    }
    fn ptr(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.storage.as_mut_ptr().cast()
    }
}
impl Drop for Attributes {
    fn drop(&mut self) {
        unsafe {
            if self.initialized {
                DeleteProcThreadAttributeList(self.ptr());
            }
        }
    }
}

pub struct OwnedBackend {
    job: Option<OwnedHandle>,
    process: OwnedHandle,
    pid: u32,
    logger: Option<JoinHandle<io::Result<()>>>,
}
impl OwnedBackend {
    /// Low-level process owner for fixtures/tools. Production uses start(Config, log).
    pub fn spawn(executable: &Path, args: &[OsString], log: &Path) -> Result<Self> {
        Self::spawn_with(executable, args, log, |process, job| unsafe {
            AssignProcessToJobObject(job, process)
        })
    }
    /// Hash pin, port preflight, then suspended creation and job assignment.
    pub fn start(config: &Config, log: &Path) -> Result<Self> {
        let config = config
            .clone()
            .validate()
            .map_err(|e| BackendError::new(ErrorCode::InvalidConfig, e))?;
        let _locked_executable =
            backend::verified_file(&config.backend_exe, &config.backend_sha256)?;
        backend::ensure_port_available(backend::ADDRESS.parse().unwrap())?;
        Self::spawn(&config.backend_exe, &backend::arguments(&config), log)
    }
    fn spawn_with(
        executable: &Path,
        args: &[OsString],
        log: &Path,
        assign: impl FnOnce(HANDLE, HANDLE) -> i32,
    ) -> Result<Self> {
        Self::spawn_with_cwd(executable, args, log, None, assign)
    }
    pub(crate) fn spawn_in(
        executable: &Path,
        args: &[OsString],
        log: &Path,
        cwd: &Path,
    ) -> Result<Self> {
        Self::spawn_with_cwd(executable, args, log, Some(cwd), |process, job| unsafe {
            AssignProcessToJobObject(job, process)
        })
    }
    fn spawn_with_cwd(
        executable: &Path,
        args: &[OsString],
        log: &Path,
        cwd: Option<&Path>,
        assign: impl FnOnce(HANDLE, HANDLE) -> i32,
    ) -> Result<Self> {
        let cwd_wide = cwd.map(|p| wide(p.as_os_str())).transpose()?;
        if cwd.is_some_and(|p| !p.is_absolute() || !p.is_dir()) {
            return Err(BackendError::new(
                ErrorCode::InvalidConfig,
                "invalid private cwd",
            ));
        }
        if !executable.is_absolute() || !log.is_absolute() {
            return Err(BackendError::new(
                ErrorCode::InvalidConfig,
                "backend and log paths must be absolute",
            ));
        }
        let mut command = Vec::new();
        quoted(executable.as_os_str(), &mut command)?;
        let executable = wide(executable.as_os_str())?;
        // Explicit application name prevents whitespace or executable-name ambiguity.
        for arg in args {
            command.push(32);
            quoted(arg, &mut command)?;
        }
        command.push(0);
        let log_file = File::create(log)?;
        unsafe {
            let raw_job = CreateJobObjectW(null(), null());
            if raw_job.is_null() {
                return Err(os_error());
            }
            let job = own(raw_job);
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                raw_job,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) == 0
            {
                return Err(os_error());
            }
            let sa = SECURITY_ATTRIBUTES {
                nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: null_mut(),
                bInheritHandle: 1,
            };
            let (mut read, mut write) = (null_mut(), null_mut());
            if CreatePipe(&mut read, &mut write, &sa, 65536) == 0 {
                return Err(os_error());
            }
            let read = own(read);
            let write = own(write);
            if SetHandleInformation(read.as_raw_handle(), HANDLE_FLAG_INHERIT, 0) == 0 {
                return Err(os_error());
            }
            let nul = CreateFileW(
                wide(std::ffi::OsStr::new("NUL"))?.as_ptr(),
                FILE_GENERIC_READ,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                &sa,
                OPEN_EXISTING,
                0,
                null_mut(),
            );
            if nul == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
                return Err(os_error());
            }
            let nul = own(nul);
            let inherited = [write.as_raw_handle(), nul.as_raw_handle()];
            let mut attributes = Attributes::new(&inherited)?;
            let mut startup: STARTUPINFOEXW = zeroed();
            startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
            startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
            startup.StartupInfo.hStdInput = nul.as_raw_handle();
            startup.StartupInfo.hStdOutput = write.as_raw_handle();
            startup.StartupInfo.hStdError = write.as_raw_handle();
            startup.lpAttributeList = attributes.ptr();
            let mut info: PROCESS_INFORMATION = zeroed();
            if CreateProcessW(
                executable.as_ptr(),
                command.as_mut_ptr(),
                null(),
                null(),
                1,
                CREATE_SUSPENDED | CREATE_NO_WINDOW | EXTENDED_STARTUPINFO_PRESENT,
                null(),
                cwd_wide.as_ref().map_or(null(), |v| v.as_ptr()),
                &startup.StartupInfo,
                &mut info,
            ) == 0
            {
                return Err(os_error());
            }
            let process = own(info.hProcess);
            let primary_thread = own(info.hThread);
            if assign(info.hProcess, raw_job) == 0 {
                let error = os_error();
                TerminateProcess(info.hProcess, 1);
                WaitForSingleObject(info.hProcess, 5000);
                return Err(error);
            }
            if ResumeThread(info.hThread) == u32::MAX {
                let error = os_error();
                TerminateJobObject(raw_job, 1);
                WaitForSingleObject(info.hProcess, 5000);
                return Err(error);
            }
            drop(primary_thread);
            drop(write);
            drop(nul);
            let reader = File::from(read);
            let log_path = log.to_owned();
            let logger = thread::Builder::new()
                .name("earth-backend-log".into())
                .spawn(move || drain(reader, log_file, &log_path))?;
            Ok(Self {
                job: Some(job),
                process,
                pid: info.dwProcessId,
                logger: Some(logger),
            })
        }
    }
    pub fn pid(&self) -> u32 {
        self.pid
    }
    pub fn is_running(&self) -> Result<bool> {
        let result = unsafe { WaitForSingleObject(self.process.as_raw_handle(), 0) };
        match result {
            258 => Ok(true),
            0 => Ok(false),
            _ => Err(os_error()),
        }
    }
    fn listener(&self, address: SocketAddr) -> Result<OwnedHandle> {
        if !address.is_ipv4() || !address.ip().is_loopback() {
            return Err(BackendError::new(
                ErrorCode::InvalidConfig,
                "readiness requires IPv4 loopback",
            ));
        }
        let job = self
            .job
            .as_ref()
            .ok_or_else(|| BackendError::new(ErrorCode::BackendFailed, "backend stopped"))?;
        let pid = socket_pid(address, None)?.ok_or_else(|| {
            BackendError::new(ErrorCode::BackendReadyTimeout, "backend not listening")
        })?;
        unsafe {
            let handle = OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                0,
                pid,
            );
            if handle.is_null() {
                return Err(BackendError::new(
                    ErrorCode::PortBusy,
                    "cannot verify listener process",
                ));
            }
            let process = own(handle);
            let mut belongs = 0;
            if IsProcessInJob(handle, job.as_raw_handle(), &mut belongs) == 0
                || belongs == 0
                || WaitForSingleObject(handle, 0) != 258
            {
                return Err(BackendError::new(
                    ErrorCode::PortBusy,
                    "listener is not a live member of owned backend job",
                ));
            }
            Ok(process)
        }
    }
    pub fn probe(&self, address: SocketAddr, deadline: Instant) -> Result<Hello> {
        let before = self.listener(address)?;
        let hello = backend::probe(address, deadline, |stream| {
            let local = stream.local_addr()?;
            let pid = socket_pid(address, Some(local))?.ok_or_else(|| {
                BackendError::new(ErrorCode::PortBusy, "cannot verify connected TCP owner")
            })?;
            let job = self.job.as_ref().unwrap();
            unsafe {
                let handle = OpenProcess(
                    PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                    0,
                    pid,
                );
                if handle.is_null() {
                    return Err(BackendError::new(
                        ErrorCode::PortBusy,
                        "connected owner unavailable",
                    ));
                }
                let _process = own(handle);
                let mut belongs = 0;
                if IsProcessInJob(handle, job.as_raw_handle(), &mut belongs) == 0
                    || belongs == 0
                    || WaitForSingleObject(handle, 0) != 258
                {
                    return Err(BackendError::new(
                        ErrorCode::PortBusy,
                        "connected socket not owned by backend job",
                    ));
                }
            }
            Ok(())
        })?;
        let after = self.listener(address)?;
        // Retain the first process handle across exchange; PID reuse cannot turn
        // a terminated listener into an accepted owner.
        if unsafe { WaitForSingleObject(before.as_raw_handle(), 0) } != 258
            || unsafe { GetProcessId(before.as_raw_handle()) }
                != unsafe { GetProcessId(after.as_raw_handle()) }
        {
            return Err(BackendError::new(
                ErrorCode::PortBusy,
                "listener changed during hello",
            ));
        }
        Ok(hello)
    }
    pub fn stop(&mut self) -> Result<()> {
        // Closing the only non-inherited job handle kills the complete tree.
        self.job.take();
        if unsafe { WaitForSingleObject(self.process.as_raw_handle(), 5000) } != 0 {
            return Err(os_error());
        }
        if let Some(logger) = self.logger.take() {
            logger.join().map_err(|_| {
                BackendError::new(ErrorCode::BackendFailed, "log thread panicked")
            })??;
        }
        Ok(())
    }
}
impl Drop for OwnedBackend {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
fn rotated(path: &Path, n: u32) -> PathBuf {
    let mut p = path.as_os_str().to_os_string();
    p.push(format!(".{n}"));
    p.into()
}
fn drain(mut reader: File, mut log: File, path: &Path) -> io::Result<()> {
    const LIMIT: usize = 2 * 1024 * 1024;
    let mut size = 0;
    let mut buffer = [0; 8192];
    loop {
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            return Ok(());
        }
        let mut bytes = &buffer[..n];
        while !bytes.is_empty() {
            if size == LIMIT {
                drop(log);
                match fs::remove_file(rotated(path, 2)) {
                    Ok(()) => {}
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e),
                };
                if rotated(path, 1).exists() {
                    fs::rename(rotated(path, 1), rotated(path, 2))?;
                }
                fs::rename(path, rotated(path, 1))?;
                log = File::create(path)?;
                size = 0;
            }
            let take = bytes.len().min(LIMIT - size);
            log.write_all(&bytes[..take])?;
            size += take;
            bytes = &bytes[take..];
        }
    }
}
fn socket_pid(address: SocketAddr, remote: Option<SocketAddr>) -> Result<Option<u32>> {
    unsafe {
        let class = if remote.is_some() {
            TCP_TABLE_OWNER_PID_CONNECTIONS
        } else {
            TCP_TABLE_OWNER_PID_LISTENER
        };
        let mut bytes = 0;
        let first = GetExtendedTcpTable(null_mut(), &mut bytes, 0, AF_INET as u32, class, 0);
        if first != ERROR_INSUFFICIENT_BUFFER && first != 0 {
            return Err(BackendError::new(
                ErrorCode::PortBusy,
                "cannot inspect TCP owners",
            ));
        }
        for _ in 0..4 {
            let mut storage = vec![0u32; (bytes as usize).div_ceil(4)];
            let status = GetExtendedTcpTable(
                storage.as_mut_ptr().cast(),
                &mut bytes,
                0,
                AF_INET as u32,
                class,
                0,
            );
            if status == ERROR_INSUFFICIENT_BUFFER {
                continue;
            }
            if status != 0 {
                return Err(BackendError::new(
                    ErrorCode::PortBusy,
                    "cannot inspect TCP owners",
                ));
            }
            let count = storage[0] as usize;
            if 4 + count * size_of::<MIB_TCPROW_OWNER_PID>() > bytes as usize {
                return Err(BackendError::new(
                    ErrorCode::PortBusy,
                    "invalid TCP owner table",
                ));
            }
            let rows = std::slice::from_raw_parts(
                storage.as_ptr().add(1).cast::<MIB_TCPROW_OWNER_PID>(),
                count,
            );
            let ip = match address.ip() {
                std::net::IpAddr::V4(ip) => u32::from_ne_bytes(ip.octets()),
                _ => unreachable!(),
            };
            let mut found = None;
            for row in rows {
                if u16::from_be(row.dwLocalPort as u16) == address.port()
                    && (row.dwLocalAddr == ip || row.dwLocalAddr == 0)
                    && remote.map_or(true, |r| {
                        u16::from_be(row.dwRemotePort as u16) == r.port()
                            && row.dwRemoteAddr == u32::from_ne_bytes([127, 0, 0, 1])
                            && row.dwState == 5
                    })
                {
                    if found.is_some() {
                        return Err(BackendError::new(
                            ErrorCode::PortBusy,
                            "ambiguous TCP listeners",
                        ));
                    }
                    found = Some(row.dwOwningPid);
                }
            }
            return Ok(found);
        }
        Err(BackendError::new(
            ErrorCode::PortBusy,
            "TCP owner table changed repeatedly",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    struct Cleanup(OwnedHandle);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            unsafe {
                TerminateProcess(self.0.as_raw_handle(), 1);
                WaitForSingleObject(self.0.as_raw_handle(), 5000);
            }
        }
    }
    #[test]
    fn assign_process_failure_terminates_suspended_child_before_returning() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../artifacts/backend-assignment");
        fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let exe = std::env::current_exe().unwrap();
        let captured = RefCell::new(None);
        let event = unsafe { own(CreateEventW(null(), 0, 0, null())) };
        let result = OwnedBackend::spawn_with(
            &exe,
            &[],
            &root.join("server.log"),
            |process, _job| unsafe {
                let handle = OpenProcess(
                    PROCESS_TERMINATE | PROCESS_SYNCHRONIZE,
                    0,
                    GetProcessId(process),
                );
                assert!(!handle.is_null());
                *captured.borrow_mut() = Some(Cleanup(own(handle)));
                // Real API failure: an Event is not a Job Object. No fixture is resumed.
                AssignProcessToJobObject(event.as_raw_handle(), process)
            },
        );
        assert!(result.is_err());
        let process = captured.borrow();
        let process = process.as_ref().unwrap();
        assert_eq!(
            unsafe { WaitForSingleObject(process.0.as_raw_handle(), 500) },
            0,
            "suspended process survived assignment failure"
        );
    }
}
