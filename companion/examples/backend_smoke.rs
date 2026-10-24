//! Isolated official-backend smoke tool. It never launches a game.
#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use beamng_earth_companion::{backend, config::OFFICIAL_BACKEND_SHA256, job::OwnedBackend};
    use std::{
        ffi::OsString,
        fs,
        path::PathBuf,
        thread,
        time::{Duration, Instant},
    };
    let mut input = std::env::args_os().skip(1);
    let exe = PathBuf::from(input.next().ok_or("missing backend")?);
    let root = PathBuf::from(input.next().ok_or("missing fresh output")?);
    if input.next().is_some() {
        return Err("unexpected arguments".into());
    }
    let root = root.canonicalize()?;
    let artifacts = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../artifacts")
        .canonicalize()?;
    if !root.starts_with(artifacts)
        || fs::read_dir(root.join("profile"))?.next().is_some()
        || fs::read_dir(root.join("cache"))?.next().is_some()
    {
        return Err("smoke requires fresh profile and cache under repository artifacts".into());
    }
    backend::verify_hash(&exe, OFFICIAL_BACKEND_SHA256)?;
    let mut args: Vec<OsString> = ["--server", "--port", "0", "--user-dir"]
        .into_iter()
        .map(Into::into)
        .collect();
    args.push(root.join("profile").into());
    args.push("--cache-dir".into());
    args.push(root.join("cache").into());
    // Exact isolated offline settings from the audited smoke_earth153_package.py.
    args.extend(
        [
            "--offline",
            "--update-check",
            "off",
            "--map-tiles",
            "off",
            "--weather",
            "off",
            "--gravity",
            "off",
            "--imagery",
            "off",
            "--roads",
            "off",
            "--surface",
            "off",
            "--airports",
            "off",
            "--sea",
            "off",
            "--trees",
            "off",
            "--buildings",
            "off",
            "--traffic",
            "off",
            "--block-process",
            "off",
            "--workers",
            "1",
            "--no-horizon",
            "--download-ahead",
            "off",
            "--pin",
            "off",
            "--nice",
            "0",
        ]
        .into_iter()
        .map(OsString::from),
    );
    let log = root.join("server.log");
    let mut owner = OwnedBackend::spawn(&exe, &args, &log)?;
    println!("Owned backend root PID {}", owner.pid());
    let deadline = Instant::now() + Duration::from_secs(45);
    let hello = loop {
        if !owner.is_running()? {
            return Err("backend exited before readiness".into());
        }
        if Instant::now() >= deadline {
            return Err("backend readiness exceeded 45 seconds".into());
        }
        let text = fs::read_to_string(&log).unwrap_or_default();
        if let Some(port) = text
            .split("earthd listening on 127.0.0.1:")
            .nth(1)
            .and_then(|v| v.split_whitespace().next())
            .and_then(|v| v.trim().parse::<u16>().ok())
        {
            match owner.probe(
                format!("127.0.0.1:{port}").parse()?,
                (Instant::now() + Duration::from_millis(250)).min(deadline),
            ) {
                Ok(hello) => break hello,
                Err(error)
                    if error.code()
                        == beamng_earth_companion::protocol::ErrorCode::BackendReadyTimeout => {}
                Err(error) => return Err(error.into()),
            }
        }
        thread::sleep(Duration::from_millis(50));
    };
    let root_pid = owner.pid();
    owner.stop()?;
    if owner.is_running()? {
        return Err("backend still running after stop".into());
    }
    fs::write(
        root.join("helper-result.json"),
        serde_json::to_vec_pretty(
            &serde_json::json!({"rootPid":root_pid,"hello":hello,"stopped":true,"sentPose":false}),
        )?,
    )?;
    Ok(())
}
#[cfg(not(windows))]
fn main() {
    panic!("Windows-only smoke tool");
}
