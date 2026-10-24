#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
fn main() {
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--check-config")) {
        if std::env::args_os().count() != 2 {
            std::process::exit(2);
        }
        let result = std::env::current_exe()
            .map_err(|e| e.to_string())
            .and_then(|exe| {
                beamng_earth_companion::config::Config::load(&exe.with_file_name("config.json"))
                    .map_err(|e| e.to_string())
            })
            .and_then(|config| {
                beamng_earth_companion::backend::verify_hash(
                    &config.backend_exe,
                    &config.backend_sha256,
                )
                .map_err(|e| e.to_string())
            });
        if let Err(error) = result {
            eprintln!("helper configuration rejected: {error}");
            std::process::exit(1);
        }
        std::process::exit(0);
    }
    use beamng_earth_companion::{
        config::Config,
        game::{GameCommand, GameSession},
        protocol::ErrorCode,
    };
    let result = (|| {
        let exe = std::env::current_exe().map_err(|_| ErrorCode::InvalidConfig)?;
        let config = Config::load(&exe.with_file_name("config.json"))
            .map_err(|_| ErrorCode::InvalidConfig)?;
        let mut args = std::env::args_os().skip(1).peekable();
        let skip_update = args.peek().is_some_and(|a| a == "--skip-update");
        if skip_update { args.next(); }
        let command = GameCommand::parse(args).map_err(|e| e.code())?;
        // Network failures resume the game inside the worker's update lock.
        // Never bypass that lock on a damaged/mid-replacement worker image.
        if !skip_update && beamng_earth_companion::updates::handoff(&exe, &config, &command)
            .map_err(|_| ErrorCode::InvalidConfig)? {
            return Ok(());
        }
        let game = GameSession::launch(&command, &config.game_root).map_err(|e| e.code())?;
        beamng_earth_companion::runtime::run(config, game).map_err(|e| e.code())
    })();
    if let Err(code) = result {
        let message = if code == ErrorCode::GameConflict {
            "Игра или помощник уже запущены. Закройте предыдущую сессию перед повторным запуском."
        } else if code == ErrorCode::ProfileUnavailable {
            "Помощник остановлен: профиль игры недоступен для записи. Проверьте доступ к настроенному профилю."
        } else {
            "Не удалось запустить игру. Проверьте config.json рядом с помощником и доступность настроенного профиля и файлов."
        };
        show_error(message);
        std::process::exit(1);
    }
}

#[cfg(windows)]
fn show_error(message: &str) {
    use std::{ffi::OsStr, os::windows::ffi::OsStrExt, ptr};
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
    let message: Vec<_> = OsStr::new(message).encode_wide().chain(Some(0)).collect();
    let title: Vec<_> = OsStr::new("BeamNG Earth")
        .encode_wide()
        .chain(Some(0))
        .collect();
    unsafe {
        MessageBoxW(
            ptr::null_mut(),
            message.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("This companion requires Windows x64.");
}
