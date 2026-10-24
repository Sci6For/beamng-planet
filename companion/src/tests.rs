use crate::config::{Config, OFFICIAL_BACKEND_SHA256};
use crate::lifecycle::{Action, Event, Lifecycle};
use crate::protocol::{ErrorCode, Observation, RequestTracker, Status, StatusState};
use std::{fs, path::PathBuf, time::Duration};

fn request(session: &str, sequence: u64, state: &str, level: &str) -> Vec<u8> {
    format!(r#"{{"formatVersion":3,"session":"{session}","sequence":{sequence},"state":"{state}","levelId":"{level}","chooserToken":"","launchId":""}}"#).into_bytes()
}

// Replaying or rolling back a sequence must not permit starting or stopping.
#[test]
fn requests_require_sequence_progress_after_each_session_baseline() {
    let mut tracker = RequestTracker::default();
    assert_eq!(
        tracker.observe(&request("A", 10, "active", "beamng_earth")),
        Observation::Baseline
    );
    assert_eq!(
        tracker.observe(&request("A", 10, "active", "beamng_earth")),
        Observation::Ignored
    );
    assert_eq!(
        tracker.observe(&request("A", 9, "idle", "")),
        Observation::Ignored
    );
    assert_eq!(
        tracker.observe(&request("A", 11, "active", "beamng_earth")),
        Observation::Active
    );
    assert_eq!(
        tracker.observe(&request("B", 40, "idle", "")),
        Observation::Baseline
    );
    assert_eq!(
        tracker.observe(&request("B", 41, "idle", "")),
        Observation::Idle
    );
}

// Bad or truncated writes must never advance the accepted baseline.
#[test]
fn malformed_requests_are_ignored_without_changing_tracker() {
    let mut tracker = RequestTracker::default();
    tracker.observe(&request("A", 10, "active", "beamng_earth"));
    let valid = request("A", 11, "active", "beamng_earth");
    let cases = vec![
        vec![b' '; 16 * 1024 + 1],
        String::from_utf8(valid.clone())
            .unwrap()
            .replace("\"formatVersion\":3", "\"formatVersion\":1")
            .into_bytes(),
        request("A", 11, "active", "other_map"),
        request("A", 11, "idle", "beamng_earth"),
        request("", 11, "active", "beamng_earth"),
        valid[..valid.len() - 1].to_vec(),
        String::from_utf8(valid.clone())
            .unwrap()
            .replace("\"levelId\"", "\"command\":\"calc.exe\",\"levelId\"")
            .into_bytes(),
    ];
    for bytes in cases {
        assert_eq!(tracker.observe(&bytes), Observation::Ignored);
    }
    assert_eq!(tracker.observe(&valid), Observation::Active);
}

#[test]
fn status_json_uses_fixed_camel_case_protocol() {
    let status = Status {
        lan: None,
        format_version: 3,
        session: "A".into(),
        state: StatusState::Error,
        error_code: Some(ErrorCode::BackendReadyTimeout),
    };
    assert_eq!(
        serde_json::to_value(status).unwrap(),
        serde_json::json!({"formatVersion":3,"session":"A","state":"error","errorCode":"backendReadyTimeout"})
    );
    assert!(serde_json::from_str::<Status>(
        r#"{"formatVersion":3,"session":"A","state":"error","errorCode":"run-cmd"}"#
    )
    .is_err());
}

fn at(seconds: u64) -> Duration {
    Duration::from_secs(seconds)
}
fn has(actions: &[Action], target: Action) -> bool {
    actions.contains(&target)
}
fn running() -> Lifecycle {
    let mut lifecycle = Lifecycle::default();
    assert!(has(
        &lifecycle.step(Event::Active, at(0)),
        Action::StartBackend
    ));
    assert!(has(
        &lifecycle.step(Event::BackendReady, at(1)),
        Action::Publish {
            state: StatusState::Ready,
            error_code: None
        }
    ));
    lifecycle
}

// An explicit idle starts one grace period; duplicate idle does not extend it.
#[test]
fn idle_stops_backend_at_twenty_seconds() {
    let mut lifecycle = running();
    assert!(lifecycle.step(Event::Idle, at(10)).is_empty());
    assert!(lifecycle.step(Event::Idle, at(20)).is_empty());
    assert!(lifecycle.step(Event::Tick, at(29)).is_empty());
    let actions = lifecycle.step(Event::Tick, at(30));
    assert!(has(&actions, Action::StopBackend));
    assert!(has(
        &actions,
        Action::Publish {
            state: StatusState::Waiting,
            error_code: None
        }
    ));
    assert!(lifecycle.step(Event::Tick, at(31)).is_empty());
}

#[test]
fn active_cancels_idle_and_missing_heartbeats_never_stop_backend() {
    let mut lifecycle = running();
    lifecycle.step(Event::Idle, at(10));
    assert!(lifecycle.step(Event::Active, at(29)).is_empty());
    assert!(lifecycle.step(Event::Tick, at(10000)).is_empty());
}

#[test]
fn readiness_deadline_stops_failed_start_and_retries_after_two_seconds() {
    let mut lifecycle = Lifecycle::default();
    lifecycle.step(Event::Active, at(0));
    assert!(lifecycle.step(Event::Tick, at(44)).is_empty());
    let actions = lifecycle.step(Event::Tick, at(45));
    assert!(has(&actions, Action::StopBackend));
    assert!(has(
        &actions,
        Action::Publish {
            state: StatusState::Error,
            error_code: Some(ErrorCode::BackendReadyTimeout)
        }
    ));
    assert!(lifecycle.step(Event::Tick, at(46)).is_empty());
    assert!(has(
        &lifecycle.step(Event::Tick, at(47)),
        Action::StartBackend
    ));
}

#[test]
fn retry_delays_and_sliding_window_limit_all_starts() {
    let mut lifecycle = running();
    lifecycle.step(Event::BackendFailed(ErrorCode::BackendFailed), at(10));
    assert!(lifecycle.step(Event::Tick, at(11)).is_empty());
    assert!(has(
        &lifecycle.step(Event::Tick, at(12)),
        Action::StartBackend
    ));
    lifecycle.step(Event::BackendFailed(ErrorCode::BackendFailed), at(13));
    assert!(lifecycle.step(Event::Tick, at(22)).is_empty());
    assert!(has(
        &lifecycle.step(Event::Tick, at(23)),
        Action::StartBackend
    ));
    lifecycle.step(Event::BackendFailed(ErrorCode::BackendFailed), at(24));
    assert!(lifecycle.step(Event::Tick, at(53)).is_empty());
    assert!(has(
        &lifecycle.step(Event::Tick, at(54)),
        Action::StartBackend
    ));
    let actions = lifecycle.step(Event::BackendFailed(ErrorCode::BackendFailed), at(55));
    assert!(has(
        &actions,
        Action::Publish {
            state: StatusState::Error,
            error_code: Some(ErrorCode::RetryLimit)
        }
    ));
    assert!(lifecycle.step(Event::Active, at(311)).is_empty());
    assert!(lifecycle.step(Event::Tick, at(311)).is_empty());
    lifecycle.step(Event::Active, at(312));
    assert!(!has(
        &lifecycle.step(Event::Tick, at(341)),
        Action::StartBackend
    ));
    assert!(has(
        &lifecycle.step(Event::Tick, at(342)),
        Action::StartBackend
    ));
}

#[test]
fn leaving_map_cancels_retries_and_late_readiness_is_ignored() {
    let mut lifecycle = running();
    lifecycle.step(Event::BackendFailed(ErrorCode::BackendFailed), at(10));
    lifecycle.step(Event::Idle, at(11));
    assert!(lifecycle.step(Event::BackendReady, at(12)).is_empty());
    assert!(lifecycle.step(Event::Tick, at(1000)).is_empty());
}

// A map transition must not turn a failed automatic restart into a free initial start.
#[test]
fn idle_active_transition_preserves_retry_delay_and_failure_budget() {
    let mut lifecycle = running();
    lifecycle.step(Event::BackendFailed(ErrorCode::BackendFailed), at(10));
    lifecycle.step(Event::Idle, at(11));
    assert!(!has(
        &lifecycle.step(Event::Active, at(12)),
        Action::StartBackend
    ));
    assert!(lifecycle.step(Event::Active, at(13)).is_empty());
    assert!(has(
        &lifecycle.step(Event::Tick, at(14)),
        Action::StartBackend
    ));
    lifecycle.step(Event::BackendFailed(ErrorCode::BackendFailed), at(15));
    assert!(lifecycle.step(Event::Active, at(16)).is_empty());
    assert!(lifecycle.step(Event::Active, at(24)).is_empty());
    assert!(has(
        &lifecycle.step(Event::Tick, at(25)),
        Action::StartBackend
    ));
}

#[test]
fn stopping_an_unready_retry_on_idle_keeps_next_start_subject_to_budget() {
    let mut lifecycle = running();
    lifecycle.step(Event::BackendFailed(ErrorCode::BackendFailed), at(10));
    assert!(has(
        &lifecycle.step(Event::Tick, at(12)),
        Action::StartBackend
    ));
    lifecycle.step(Event::Idle, at(13));
    assert!(has(
        &lifecycle.step(Event::Tick, at(33)),
        Action::StopBackend
    ));
    assert!(!has(
        &lifecycle.step(Event::Active, at(34)),
        Action::StartBackend
    ));
    assert!(lifecycle.step(Event::Tick, at(43)).is_empty());
    assert!(has(
        &lifecycle.step(Event::Tick, at(44)),
        Action::StartBackend
    ));
}

#[test]
fn game_exit_always_stops_and_exits_and_cancels_future_events() {
    for mut lifecycle in [Lifecycle::default(), running()] {
        let actions = lifecycle.step(Event::GameExited, at(10));
        assert!(has(&actions, Action::StopBackend));
        assert!(has(&actions, Action::Exit));
        assert!(lifecycle.step(Event::Active, at(11)).is_empty());
        assert!(lifecycle.step(Event::Tick, at(1000)).is_empty());
    }
    let mut retrying = running();
    retrying.step(Event::BackendFailed(ErrorCode::BackendFailed), at(10));
    assert!(has(&retrying.step(Event::GameExited, at(11)), Action::Exit));
    assert!(retrying.step(Event::Tick, at(12)).is_empty());
}

#[test]
fn backward_time_does_not_move_idle_deadline() {
    let mut lifecycle = running();
    lifecycle.step(Event::Idle, at(10));
    assert!(lifecycle.step(Event::Tick, at(5)).is_empty());
    assert!(lifecycle.step(Event::Tick, at(29)).is_empty());
    assert!(has(
        &lifecycle.step(Event::Tick, at(30)),
        Action::StopBackend
    ));
}

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "earth-config-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        for directory in ["game", "profile", "cache"] {
            fs::create_dir_all(root.join(directory)).unwrap();
        }
        fs::write(root.join("backend.exe"), b"fixture; never executed").unwrap();
        Self(root)
    }
    fn config(&self) -> Config {
        Config {
            game_root: self.0.join("game"),
            profile: self.0.join("profile"),
            cache_dir: self.0.join("cache"),
            backend_exe: self.0.join("backend.exe"),
            backend_sha256: option_env!("PLANET_BACKEND_SHA256")
                .unwrap_or(OFFICIAL_BACKEND_SHA256)
                .into(),
        }
    }
}

#[test]
fn build_time_backend_pin_accepts_only_its_own_hash() {
    let fixture = Fixture::new();
    let mut config = fixture.config();
    assert!(config.clone().validate().is_ok());
    config.backend_sha256 = if config.backend_sha256 == OFFICIAL_BACKEND_SHA256 {
        "0".repeat(64)
    } else {
        OFFICIAL_BACKEND_SHA256.into()
    };
    assert!(config.validate().is_err());
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn configuration_validates_absolute_existing_paths_and_preserves_profile() {
    let fixture = Fixture::new();
    let config = fixture.config().validate().unwrap();
    assert_eq!(
        config.profile,
        fixture.0.join("profile").canonicalize().unwrap()
    );
    assert_eq!(
        config.request_path(),
        config
            .profile
            .join("settings/beamngEarthAutostart/request.json")
    );
    assert_eq!(
        config.status_path(),
        config
            .profile
            .join("settings/beamngEarthAutostart/status.json")
    );
    let mut config = fixture.config();
    config.profile = fixture.0.join("stale-profile");
    assert!(config.validate().is_err());
    let mut config = fixture.config();
    config.profile = PathBuf::from("profile");
    assert!(config.validate().is_err());
    let mut config = fixture.config();
    config.backend_sha256 = "0".repeat(64);
    assert!(config.validate().is_err());
}

#[test]
fn config_file_rejects_command_fields_and_wrong_path_types() {
    let fixture = Fixture::new();
    let path = fixture.0.join("config.json");
    fs::write(&path, serde_json::to_vec(&fixture.config()).unwrap()).unwrap();
    assert!(Config::load(&path).is_ok());
    let mut json = serde_json::to_value(fixture.config()).unwrap();
    json["arguments"] = serde_json::json!(["--arbitrary-command"]);
    fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
    assert!(Config::load(&path).is_err());
    let mut config = fixture.config();
    config.backend_exe = config.profile.clone();
    assert!(config.validate().is_err());
    let mut config = fixture.config();
    config.cache_dir = config.backend_exe.clone();
    assert!(config.validate().is_err());
}
