//! Finish the LAN firewall permission before the game can begin loading a world.
use crate::{backend, config::Config};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    ffi::OsStr,
    fs::File,
    io::Read,
    os::windows::{ffi::OsStrExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    MessageBoxW, IDOK, IDRETRY, MB_ICONINFORMATION, MB_ICONWARNING, MB_OKCANCEL, MB_RETRYCANCEL,
};

const BLOCKED: &str = "Входящие соединения Planet запрещены существующим правилом или политикой Windows. Разрешите установленному kissmp-server.exe входящий UDP 3698 в локальной сети либо обратитесь к администратору. Planet не изменяет существующие запреты.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PermissionState {
    Ready,
    NeedsSetup,
    Blocked,
}

trait PermissionSetup {
    fn check(&mut self) -> Result<PermissionState, String>;
    fn approve(&mut self) -> bool;
    fn setup(&mut self) -> Result<(), String>;
    fn retry(&mut self, reason: &str) -> bool;
}

fn prepare(driver: &mut impl PermissionSetup) -> bool {
    loop {
        let error = match driver.check() {
            Ok(PermissionState::Ready) => return true,
            Ok(PermissionState::Blocked) => BLOCKED.into(),
            Err(error) => error,
            Ok(PermissionState::NeedsSetup) => {
                if !driver.approve() {
                    return false;
                }
                match driver.setup() {
                    Err(error) => error,
                    Ok(()) => match driver.check() {
                        Ok(PermissionState::Ready) => return true,
                        Ok(PermissionState::Blocked) => BLOCKED.into(),
                        Ok(PermissionState::NeedsSetup) => {
                            "Windows не сохранила разрешение локальной сети.".into()
                        }
                        Err(error) => error,
                    },
                }
            }
        };
        if !driver.retry(&error) {
            return false;
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServerImage {
    server_exe: PathBuf,
    server_sha256: String,
}

struct WindowsSetup {
    server: String,
    sha256: String,
    rule_name: String,
    // Keep the image unreplaceable while verifying it and asking for permission.
    _pin: File,
}

fn windows_path(path: &Path) -> Result<String, String> {
    let text = path
        .to_str()
        .ok_or("Путь к серверу содержит неподдерживаемые символы.")?;
    Ok(if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{unc}")
    } else {
        text.strip_prefix(r"\\?\").unwrap_or(text).to_owned()
    })
}

impl WindowsSetup {
    fn load(config: &Config) -> Result<Self, String> {
        let mut bytes = Vec::new();
        File::open(config.lan_inventory_path())
            .map_err(|e| e.to_string())?
            .take(16385)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > 16384 {
            return Err("Повреждён список сетевых файлов Planet.".into());
        }
        let image: ServerImage = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        let actual = image.server_exe.canonicalize().map_err(|e| e.to_string())?;
        // Never elevate a path selected by a peer, world request, or altered inventory.
        let expected = config
            .backend_exe
            .parent()
            .and_then(Path::parent)
            .ok_or("Повреждён путь установки Planet.")?
            .join("network/kissmp-server.exe")
            .canonicalize()
            .map_err(|e| e.to_string())?;
        if actual != expected {
            return Err("Сетевой сервер находится вне установки Planet.".into());
        }
        let pin =
            backend::verified_file(&actual, &image.server_sha256).map_err(|e| e.to_string())?;
        let server = windows_path(&actual)?;
        let identity = format!("{:x}", Sha256::digest(server.to_lowercase().as_bytes()));
        Ok(Self {
            server,
            sha256: image.server_sha256,
            rule_name: format!("Planet LAN UDP 3698 {}", &identity[..16]),
            _pin: pin,
        })
    }

    fn script(&self, setup: bool) -> String {
        let operation = if setup {
            "Set-PlanetNetworkPermission"
        } else {
            "Get-PlanetNetworkState"
        };
        format!("$ErrorActionPreference = 'Stop'\n{}\ntry {{\n$application = {}\nif ((Get-FileHash -LiteralPath $application -Algorithm SHA256).Hash -ne {}) {{ exit 30 }}\n$policy = New-Object -ComObject HNetCfg.FwPolicy2\n$state = {} $policy $application {}\nswitch ($state) {{ 'Ready' {{ exit 0 }} 'NeedsSetup' {{ exit 10 }} 'Blocked' {{ exit 20 }} default {{ exit 30 }} }}\n}} catch {{ exit 30 }}",
            include_str!("../assets/planet_network.ps1"), ps_literal(&self.server), ps_literal(&self.sha256), operation, ps_literal(&self.rule_name))
    }
}

fn ps_literal(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

fn powershell() -> Result<PathBuf, String> {
    std::env::var_os("SystemRoot")
        .map(|p| PathBuf::from(p).join("System32/WindowsPowerShell/v1.0/powershell.exe"))
        .ok_or_else(|| "Не найден Windows PowerShell.".into())
}

fn run_script(script: &str, timeout: Duration) -> Result<i32, String> {
    let mut child = Command::new(powershell()?)
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            script,
        ])
        .creation_flags(0x08000000)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Не удалось проверить разрешение Windows: {e}"))?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status.code().unwrap_or(30)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error.to_string());
            }
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(
                    "Windows не ответила на запрос разрешения вовремя. Повторите запрос.".into(),
                );
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

fn elevate_script(script: &str) -> Result<String, String> {
    // Elevate immutable embedded text, never a writable temporary .ps1. A timed-out
    // launcher cannot later grant a rule if its delayed UAC prompt is accepted.
    Ok(format!("$ErrorActionPreference='Stop'; try {{ $script={}; $guard='if (-not (Get-Process -Id ' + $PID + ' -ErrorAction SilentlyContinue)) {{ exit 40 }}; '; $encoded=[Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($guard + $script)); $p=Start-Process -FilePath {} -Verb RunAs -WindowStyle Hidden -PassThru -Wait -ArgumentList @('-NoProfile','-NonInteractive','-EncodedCommand',$encoded); exit $p.ExitCode }} catch {{ exit 40 }}", ps_literal(script), ps_literal(&windows_path(&powershell()?)?)))
}

impl PermissionSetup for WindowsSetup {
    fn check(&mut self) -> Result<PermissionState, String> {
        match run_script(&self.script(false), Duration::from_secs(20))? {
            0 => Ok(PermissionState::Ready),
            10 => Ok(PermissionState::NeedsSetup),
            20 => Ok(PermissionState::Blocked),
            _ => Err("Не удалось прочитать настройки брандмауэра Windows. Проверьте службу брандмауэра и целостность установки Planet.".into()),
        }
    }
    fn approve(&mut self) -> bool {
        message("Planet подготовит доступ для локальной игры до запуска BeamNG.drive.\n\nПодтвердите следующий запрос Windows: будет разрешён только установленный сервер Planet, UDP 3698, только для устройств вашей локальной сети. Другие порты настраиваются отдельно.\n\nOK — продолжить; Отмена — завершить запуск игры.", MB_OKCANCEL | MB_ICONINFORMATION) == IDOK
    }
    fn setup(&mut self) -> Result<(), String> {
        match run_script(&elevate_script(&self.script(true))?, Duration::from_secs(180))? {
            0 => Ok(()),
            20 => Err(BLOCKED.into()),
            40 => Err("Разрешение Windows не подтверждено. Нажмите «Повторить», чтобы запросить его снова, или «Отмена», чтобы завершить запуск игры.".into()),
            _ => Err("Не удалось сохранить разрешение локальной сети. Проверьте права администратора и настройки брандмауэра Windows.".into()),
        }
    }
    fn retry(&mut self, reason: &str) -> bool {
        retry(reason)
    }
}

fn message(text: &str, style: u32) -> i32 {
    let text: Vec<_> = OsStr::new(text).encode_wide().chain(Some(0)).collect();
    let title: Vec<_> = OsStr::new("Planet — доступ к локальной сети")
        .encode_wide()
        .chain(Some(0))
        .collect();
    unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), style) }
}
fn retry(reason: &str) -> bool {
    message(&format!("{reason}\n\nИгра ещё не запущена. «Повторить» проверит разрешение заново; «Отмена» завершит запуск."), MB_RETRYCANCEL | MB_ICONWARNING) == IDRETRY
}

/// False is a clean user cancellation: the caller must not launch the game.
pub fn ensure(config: &Config) -> bool {
    loop {
        match WindowsSetup::load(config) {
            Ok(mut driver) => return prepare(&mut driver),
            Err(error) => {
                if !retry(&format!(
                    "Не удалось подготовить сетевые файлы Planet: {error}"
                )) {
                    return false;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    struct Driver {
        states: VecDeque<Result<PermissionState, String>>,
        approve: bool,
        result: Result<(), String>,
        retry: bool,
        events: Vec<&'static str>,
    }
    impl PermissionSetup for Driver {
        fn check(&mut self) -> Result<PermissionState, String> {
            self.events.push("check");
            self.states.pop_front().expect("unexpected repeat")
        }
        fn approve(&mut self) -> bool {
            self.events.push("approve");
            self.approve
        }
        fn setup(&mut self) -> Result<(), String> {
            self.events.push("setup");
            self.result.clone()
        }
        fn retry(&mut self, _: &str) -> bool {
            self.events.push("retry");
            self.retry
        }
    }
    fn driver(states: &[PermissionState]) -> Driver {
        Driver {
            states: states.iter().copied().map(Ok).collect(),
            approve: true,
            result: Ok(()),
            retry: false,
            events: vec![],
        }
    }
    #[test]
    fn cancelled_permission_prevents_game_launch() {
        let mut d = driver(&[PermissionState::NeedsSetup]);
        d.approve = false;
        assert!(!prepare(&mut d));
        assert_eq!(d.events, ["check", "approve"]);
    }
    #[test]
    fn existing_permission_never_prompts_or_elevates() {
        let mut d = driver(&[PermissionState::Ready]);
        assert!(prepare(&mut d));
        assert_eq!(d.events, ["check"]);
    }
    #[test]
    fn setup_is_rechecked_before_allowing_game_launch() {
        let mut d = driver(&[PermissionState::NeedsSetup, PermissionState::Ready]);
        assert!(prepare(&mut d));
        assert_eq!(d.events, ["check", "approve", "setup", "check"]);
    }
    #[test]
    fn rejected_uac_does_not_launch_game_and_can_be_cancelled() {
        let mut d = driver(&[PermissionState::NeedsSetup]);
        d.result = Err("cancelled".into());
        assert!(!prepare(&mut d));
        assert_eq!(d.events, ["check", "approve", "setup", "retry"]);
    }
    #[test]
    fn user_policy_is_not_overridden_by_setup() {
        let mut d = driver(&[PermissionState::Blocked]);
        assert!(!prepare(&mut d));
        assert_eq!(d.events, ["check", "retry"]);
    }
    #[test]
    fn explicit_retry_rechecks_policy_before_setup() {
        let mut d = driver(&[PermissionState::Blocked, PermissionState::Ready]);
        d.retry = true;
        assert!(prepare(&mut d));
        assert_eq!(d.events, ["check", "retry", "check"]);
    }
    #[test]
    fn ineffective_setup_requires_explicit_retry_instead_of_repeated_uac() {
        let mut d = driver(&[PermissionState::NeedsSetup, PermissionState::NeedsSetup]);
        assert!(!prepare(&mut d));
        assert_eq!(d.events, ["check", "approve", "setup", "check", "retry"]);
    }
    #[test]
    fn firewall_rule_behavior_is_verified_without_touching_windows_rules() {
        let status = Command::new(powershell().unwrap())
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ])
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/network_preflight.ps1"))
            .creation_flags(0x08000000)
            .status()
            .unwrap();
        assert!(status.success());
    }
    #[test]
    fn powershell_keeps_special_path_characters_literal() {
        let dir = std::env::temp_dir().join(format!("planet-network-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("Planet's $(throw 'executed') & сеть.txt");
        std::fs::write(&file, "test").unwrap();
        let script = format!(
            "if (Test-Path -LiteralPath {} -PathType Leaf) {{ exit 0 }}; exit 9",
            ps_literal(file.to_str().unwrap())
        );
        let result = run_script(&script, Duration::from_secs(10));
        let _ = std::fs::remove_dir_all(dir);
        assert_eq!(result.unwrap(), 0);
    }
    #[test]
    fn elevation_payload_is_encoded_and_hidden_without_using_a_writable_script() {
        // Replace only the external elevation boundary. The real launcher still
        // builds, encodes and passes its payload through Windows PowerShell.
        let interceptor = r#"function Start-Process {
            param($FilePath, $Verb, [switch]$PassThru, [switch]$Wait, $WindowStyle, $ArgumentList)
            if ($Verb -ne 'RunAs' -or $WindowStyle -ne 'Hidden' -or -not $Wait -or -not $PassThru) { throw 'unsafe launch' }
            if ($ArgumentList[2] -ne '-EncodedCommand') { throw 'not encoded' }
            $body = [Text.Encoding]::Unicode.GetString([Convert]::FromBase64String($ArgumentList[3]))
            $tokens = $null; $errors = $null
            $null = [Management.Automation.Language.Parser]::ParseInput($body, [ref]$tokens, [ref]$errors)
            if ($errors.Count -ne 0 -or -not $body.EndsWith("exit 23")) { throw 'damaged payload' }
            [pscustomobject]@{ ExitCode = 23 }
        }"#;
        let script = format!(
            "{interceptor}\n{}",
            elevate_script("$value='сеть'; exit 23").unwrap()
        );
        assert_eq!(run_script(&script, Duration::from_secs(10)).unwrap(), 23);
    }
}
