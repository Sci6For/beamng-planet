//! Hand the Steam launch to a package-pinned worker before loading the game.
//! The worker lives outside managed payload trees so Windows can replace them.
use crate::{config::Config, game::GameCommand};
use std::{ffi::OsString, fs, io, path::Path, process::Command};
use std::os::windows::process::CommandExt;
use std::os::windows::ffi::{OsStrExt,OsStringExt};

fn installer_path(path: &Path) -> OsString {
    let value: Vec<u16> = path.as_os_str().encode_wide().collect();
    // Python's ownership records use ordinary drive/UNC spellings; Rust's
    // canonicalize prefixes paths with \\?\ without changing their identity.
    if value.starts_with(&[92,92,63,92,85,78,67,92]) {
        let mut normal=vec![92,92];normal.extend_from_slice(&value[8..]);
        OsString::from_wide(&normal)
    } else if value.starts_with(&[92,92,63,92]) {
        OsString::from_wide(&value[4..])
    } else { path.as_os_str().to_owned() }
}

pub fn worker_arguments(installation: &Path, profile: &Path, parent: u32, game: &GameCommand) -> Vec<OsString> {
    let mut result = vec!["--handoff".into(), "--installation".into(), installer_path(installation),
        "--profile".into(), installer_path(profile), "--parent-pid".into(), parent.to_string().into(), "--".into(), game.executable.clone().into()];
    result.extend(game.arguments.iter().cloned());
    result
}

pub fn handoff(exe: &Path, config: &Config, game: &GameCommand) -> io::Result<bool> {
    let Some(pin) = option_env!("PLANET_UPDATER_SHA256") else { return Ok(false) };
    let root = exe.parent().ok_or_else(|| io::Error::other("missing installation"))?;
    if !root.join("update-channel.json").is_file() { return Ok(false) }
    let source = root.join("planet-update.exe");
    crate::backend::verify_hash(&source, pin).map_err(io::Error::other)?;
    let state = config.profile.join("settings/beamngEarthUpdates");
    fs::create_dir_all(&state)?;
    // Do not follow user-created reparse points outside this profile.
    if !state.canonicalize()?.starts_with(config.profile.canonicalize()?) {
        return Err(io::Error::other("update state escapes the profile"));
    }
    let worker = state.join(format!("runner-{pin}.exe"));
    if !worker.exists() {
        let mut out = fs::OpenOptions::new().write(true).create_new(true).open(&worker)?;
        io::copy(&mut fs::File::open(&source)?, &mut out)?;
        out.sync_all()?;
    }
    crate::backend::verify_hash(&worker, pin).map_err(io::Error::other)?;
    Command::new(worker).args(worker_arguments(root, &config.profile, std::process::id(), game))
        .creation_flags(0x08000000).spawn()?;
    Ok(true)
}
