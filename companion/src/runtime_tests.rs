//! Real disk + real game/backend fixture processes; no production hash override.
use super::*;
use crate::{config::APPROVED_BACKEND_SHA256, game::GameCommand};
use std::{
    ffi::OsString,
    process::Command,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex, OnceLock,
    },
};
static SERIAL: Mutex<()> = Mutex::new(());
static FIXTURES: OnceLock<(PathBuf, PathBuf)> = OnceLock::new();
static NUMBER: AtomicUsize = AtomicUsize::new(0);
fn fixtures() -> &'static (PathBuf, PathBuf) {
    FIXTURES.get_or_init(|| {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../artifacts/runtime-build-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let build = |source: &str, name: &str| {
            let exe = dir.join(name);
            assert!(Command::new(std::env::var_os("RUSTC").unwrap())
                .arg(
                    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                        .join("../tests/fixtures")
                        .join(source)
                )
                .arg("-o")
                .arg(&exe)
                .status()
                .unwrap()
                .success());
            exe.canonicalize().unwrap()
        };
        (
            build("process_fixture.rs", "process.exe"),
            build("backend_fixture.rs", "backend.exe"),
        )
    })
}
fn setup(hold: &str) -> (Config, GameSession) {
    let fixtures = fixtures();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
        "../artifacts/runtime-loop-{}-{}",
        std::process::id(),
        NUMBER.fetch_add(1, Ordering::Relaxed)
    ));
    for p in ["game/Bin64", "profile", "cache"] {
        fs::create_dir_all(root.join(p)).unwrap();
    }
    let root = root.canonicalize().unwrap();
    fs::copy(&fixtures.0, root.join("game/Bin64/BeamNG.drive.x64.exe")).unwrap();
    let config = Config {
        game_root: root.join("game"),
        profile: root.join("profile"),
        cache_dir: root.join("cache"),
        backend_exe: fixtures.1.clone(),
        backend_sha256: APPROVED_BACKEND_SHA256.into(),
    }
    .validate()
    .unwrap();
    let game = GameSession::launch(
        &GameCommand {
            executable: config.game_root.join("Bin64/BeamNG.drive.x64.exe"),
            arguments: vec!["--fixture-hold".into(), hold.into()],
        },
        &config.game_root,
    )
    .unwrap();
    (config, game)
}
fn active(config: &Config) -> thread::JoinHandle<io::Result<()>> {
    let path = config.request_path();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    thread::spawn(move || {
        for seq in 1..=4 {
            thread::sleep(Duration::from_millis(300));
            fs::write(
                &path,
                format!(
                    r#"{{"formatVersion":3,"session":"fixture","sequence":{seq},"state":"active","levelId":"beamng_earth","chooserToken":"","launchId":""}}"#
                ),
            )?;
        }
        Ok(())
    })
}
#[test]
fn unchanged_stale_request_does_not_start_a_backend() {
    let _guard = SERIAL.lock().unwrap();
    let (config, game) = setup("1400");
    fs::create_dir_all(config.request_path().parent().unwrap()).unwrap();
    fs::write(config.request_path(),r#"{"formatVersion":3,"session":"old","sequence":900,"state":"active","levelId":"beamng_earth","chooserToken":"","launchId":""}"#).unwrap();
    let mut starts = 0;
    run_loop(config, game, |_, _| {
        starts += 1;
        panic!("stale request launched a process")
    })
    .unwrap();
    assert_eq!(starts, 0);
}
#[test]
fn missing_heartbeat_preserves_activity_and_crashed_backend_retries_then_dies_with_game() {
    let _guard = SERIAL.lock().unwrap();
    let (config, game) = setup("5200");
    let writer = active(&config);
    let mut starts = 0;
    let mut pids = Vec::new();
    run_loop(config.clone(), game, |_, log| {
        starts += 1;
        let hold = if starts == 1 { "500" } else { "60000" };
        let owned = OwnedBackend::spawn(
            &fixtures().0,
            &[OsString::from("--fixture-hold"), hold.into()],
            log,
        )?;
        pids.push(owned.pid());
        Ok(owned)
    })
    .unwrap();
    writer.join().unwrap().unwrap();
    assert_eq!(starts, 2, "crash must retry despite heartbeat silence");
    let log = fs::read_to_string(config.status_path().with_file_name("runtime.log")).unwrap();
    assert!(log.contains("BackendFailed"));
    for pid in pids {
        assert!(!live(pid), "owned backend survived game exit");
    }
}
fn live(pid: u32) -> bool {
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE},
    };
    unsafe {
        let h = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if h.is_null() {
            return false;
        }
        let running = WaitForSingleObject(h, 0) == 258;
        CloseHandle(h);
        running
    }
}
#[test]
fn slow_hello_does_not_block_game_exit_until_the_readiness_timeout() {
    let _guard = SERIAL.lock().unwrap();
    let (config, game) = setup("2200");
    let writer = active(&config);
    let root = config.profile.clone();
    let mut pid = 0;
    let mut starts = 0;
    let before = Instant::now();
    run_loop(config, game, |_, log| {
        starts += 1;
        let owned = OwnedBackend::spawn(
            &fixtures().1,
            &[
                "--dir".into(),
                root.as_os_str().into(),
                "--port".into(),
                "47613".into(),
                "--mode".into(),
                "slow".into(),
            ],
            log,
        )?;
        pid = owned.pid();
        Ok(owned)
    })
    .unwrap();
    writer.join().unwrap().unwrap();
    assert_eq!(
        starts, 1,
        "a slow response must not be treated as process death"
    );
    assert_ne!(pid, 0);
    assert!(!live(pid));
    assert!(
        before.elapsed() < Duration::from_secs(5),
        "slow probe blocked game exit"
    );
}
#[test]
fn ready_backend_survives_reload_and_stops_only_after_explicit_idle_grace() {
    let _guard = SERIAL.lock().unwrap();
    let (config, game) = setup("25000");
    let directory = config.status_path().parent().unwrap().to_path_buf();
    fs::create_dir_all(&directory).unwrap();
    let request = config.request_path();
    let status = config.status_path();
    let watcher = thread::spawn(move || {
        let origin = Instant::now();
        let write = |session: &str, sequence: u64, state: &str| {
            fs::write(&request,format!(r#"{{"formatVersion":3,"session":"{session}","sequence":{sequence},"state":"{state}","levelId":"{}","chooserToken":"","launchId":""}}"#,if state=="active" {"beamng_earth"} else {""})).unwrap()
        };
        write("initial", 1, "active");
        thread::sleep(Duration::from_millis(650));
        write("initial", 2, "active");
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            if fs::read(&status)
                .ok()
                .and_then(|b| serde_json::from_slice::<Status>(&b).ok())
                .is_some_and(|s| s.state == StatusState::Ready)
            {
                break;
            }
            assert!(Instant::now() < until, "backend never became ready");
            thread::sleep(Duration::from_millis(40));
        }
        write("reload", 1, "active");
        thread::sleep(Duration::from_millis(650));
        let new_status: Status = serde_json::from_slice(&fs::read(&status).unwrap()).unwrap();
        assert_eq!(new_status.session, "reload");
        assert_eq!(new_status.state, StatusState::Ready);
        write("reload", 2, "idle");
        thread::sleep(Duration::from_secs(19));
        let during: Status = serde_json::from_slice(&fs::read(&status).unwrap()).unwrap();
        assert_eq!(
            during.state,
            StatusState::Ready,
            "idle stopped backend before grace elapsed"
        );
        let until = Instant::now() + Duration::from_secs(3);
        loop {
            let s: Status = serde_json::from_slice(&fs::read(&status).unwrap()).unwrap();
            if s.state == StatusState::Waiting {
                break;
            }
            assert!(
                Instant::now() < until,
                "idle did not stop backend after grace"
            );
            thread::sleep(Duration::from_millis(40));
        }
        assert!(origin.elapsed() >= Duration::from_secs(20));
    });
    let root = config.profile.clone();
    let mut starts = 0;
    let mut pid = 0;
    run_loop(config.clone(), game, |_, log| {
        starts += 1;
        let owned = OwnedBackend::spawn(
            &fixtures().1,
            &[
                "--dir".into(),
                root.as_os_str().into(),
                "--port".into(),
                "47613".into(),
            ],
            log,
        )?;
        pid = owned.pid();
        Ok(owned)
    })
    .unwrap();
    watcher.join().unwrap();
    assert_eq!(starts, 1);
    assert!(!live(pid));
    let log = fs::read_to_string(directory.join("runtime.log")).unwrap();
    assert!(log.contains("Ready"));
    assert!(log.contains("Stopping"));
}

#[test]
fn ready_backend_stops_after_rapid_reload_unload_with_no_further_heartbeat() {
    use std::{
        io::{Seek, SeekFrom},
        os::windows::fs::OpenOptionsExt,
    };
    let _guard = SERIAL.lock().unwrap();
    let (config, game) = setup("26000");
    let directory = config.status_path().parent().unwrap().to_path_buf();
    fs::create_dir_all(&directory).unwrap();
    let request = config.request_path();
    let status = config.status_path();
    let profile = config.profile.clone();
    let watcher = thread::spawn(move || {
        fs::write(&request, r#"{"formatVersion":3,"session":"A","sequence":1,"state":"active","levelId":"beamng_earth","chooserToken":"","launchId":""}"#).unwrap();
        thread::sleep(Duration::from_millis(650));
        fs::write(&request, r#"{"formatVersion":3,"session":"A","sequence":2,"state":"active","levelId":"beamng_earth","chooserToken":"","launchId":""}"#).unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            if fs::read(&status)
                .ok()
                .and_then(|b| serde_json::from_slice::<Status>(&b).ok())
                .is_some_and(|s| s.session == "A" && s.state == StatusState::Ready)
            {
                break;
            }
            assert!(Instant::now() < until, "backend never became ready for A");
            thread::sleep(Duration::from_millis(20));
        }
        let pid: u32 = fs::read_to_string(profile.join("pid-0"))
            .unwrap()
            .parse()
            .unwrap();
        assert!(live(pid));
        // One short exclusive write interval ensures the runtime can observe
        // only B's terminal idle, even if the poll races these two addon writes.
        let mut file = fs::OpenOptions::new()
            .write(true)
            .share_mode(0)
            .open(&request)
            .unwrap();
        file.set_len(0).unwrap();
        file.write_all(br#"{"formatVersion":3,"session":"B","sequence":1,"state":"active","levelId":"beamng_earth","chooserToken":"","launchId":""}"#).unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();
        file.set_len(0).unwrap();
        file.write_all(
            br#"{"formatVersion":3,"session":"B","sequence":2,"state":"idle","levelId":"","chooserToken":"","launchId":""}"#,
        )
        .unwrap();
        file.sync_all().unwrap();
        drop(file);
        let idle_written = Instant::now();
        // No more request writes: the addon has unloaded.
        let until = Instant::now() + Duration::from_secs(2);
        loop {
            let s: Status = serde_json::from_slice(&fs::read(&status).unwrap()).unwrap();
            if s.session == "B" {
                assert_eq!(s.state, StatusState::Ready);
                break;
            }
            assert!(
                Instant::now() < until,
                "runtime never observed terminal B request"
            );
            thread::sleep(Duration::from_millis(20));
        }
        thread::sleep(Duration::from_secs(19).saturating_sub(idle_written.elapsed()));
        let s: Status = serde_json::from_slice(&fs::read(&status).unwrap()).unwrap();
        assert_eq!(
            s.state,
            StatusState::Ready,
            "terminal idle bypassed twenty-second grace"
        );
        assert!(live(pid), "backend stopped before grace elapsed");
        let until = idle_written + Duration::from_secs(22);
        loop {
            let s: Status = serde_json::from_slice(&fs::read(&status).unwrap()).unwrap();
            if s.state == StatusState::Waiting {
                assert_eq!(s.session, "B");
                break;
            }
            assert!(
                Instant::now() < until,
                "terminal idle was lost: backend remained ready without heartbeats"
            );
            thread::sleep(Duration::from_millis(40));
        }
        assert!(idle_written.elapsed() >= Duration::from_secs(20));
        assert!(!live(pid), "backend still alive after terminal idle grace");
    });
    let root = config.profile.clone();
    let mut starts = 0;
    run_loop(config, game, |_, log| {
        starts += 1;
        OwnedBackend::spawn(
            &fixtures().1,
            &[
                "--dir".into(),
                root.as_os_str().into(),
                "--port".into(),
                "47613".into(),
            ],
            log,
        )
    })
    .unwrap();
    watcher.join().unwrap();
    assert_eq!(
        starts, 1,
        "terminal idle must neither restart nor launch a backend"
    );
}
#[test]
fn runtime_log_rotation_is_bounded() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../artifacts/runtime-log-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let mut log = RuntimeLog::new(dir.join("runtime.log"));
    for _ in 0..4000 {
        log.write("состояние помощника изменилось").unwrap();
    }
    let files: Vec<_> = fs::read_dir(dir).unwrap().map(|p| p.unwrap()).collect();
    assert_eq!(files.len(), 2);
    assert!(files
        .iter()
        .all(|p| p.metadata().unwrap().len() <= LOG_LIMIT));
}

#[test]
fn persistent_request_access_denial_returns_visible_error_and_drops_owned_backend() {
    use std::os::windows::fs::OpenOptionsExt;
    let _guard = SERIAL.lock().unwrap();
    let (config, game) = setup("3000");
    let writer = active(&config);
    let request = config.request_path();
    let mut lock = None;
    let mut pid = 0;
    let result = run_loop(config, game, |_, log| {
        let owned = OwnedBackend::spawn(
            &fixtures().0,
            &["--fixture-hold".into(), "60000".into()],
            log,
        )?;
        pid = owned.pid();
        lock = Some(
            fs::OpenOptions::new()
                .read(true)
                .share_mode(0)
                .open(&request)
                .unwrap(),
        );
        Ok(owned)
    });
    // The writer sees the deliberately unavailable profile; join before cleanup.
    assert_eq!(writer.join().unwrap().unwrap_err().raw_os_error(), Some(32));
    drop(lock);
    assert_eq!(result.unwrap_err().code(), ErrorCode::ProfileUnavailable);
    assert_ne!(pid, 0);
    assert!(!live(pid), "owned backend survived an IPC failure");
    thread::sleep(Duration::from_millis(3000));
}
