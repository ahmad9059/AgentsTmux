use std::collections::{HashMap, HashSet};
use std::process::Command;
use std::process::Stdio;
use std::thread::sleep;
use std::time::{Duration, Instant};

use crate::time::now_epoch_secs;
use crate::tmux;

pub(crate) const DESKTOP_NOTIFICATION_COOLDOWN_SECS: u64 = 120;
const DESKTOP_NOTIFICATION_TIMEOUT: Duration = Duration::from_secs(3);
const DESKTOP_NOTIFICATION_PROBE_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DesktopNotificationKind {
    TaskCompleted,
    TaskFailed,
    PermissionRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DesktopNotificationEvent {
    Stop,
    Notification,
    TaskCompleted,
    StopFailure,
    PermissionDenied,
}

impl DesktopNotificationEvent {
    pub const ALL: [Self; 5] = [
        Self::Stop,
        Self::Notification,
        Self::TaskCompleted,
        Self::StopFailure,
        Self::PermissionDenied,
    ];

    pub const DEFAULT: [Self; 4] = [
        Self::Stop,
        Self::Notification,
        Self::StopFailure,
        Self::PermissionDenied,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stop => "stop",
            Self::Notification => "notification",
            Self::TaskCompleted => "task_completed",
            Self::StopFailure => "stop_failure",
            Self::PermissionDenied => "permission_denied",
        }
    }

    fn from_token(token: &str) -> Option<Self> {
        match token.trim().to_ascii_lowercase().as_str() {
            "stop" => Some(Self::Stop),
            "notification" => Some(Self::Notification),
            "task_completed" => Some(Self::TaskCompleted),
            "stop_failure" => Some(Self::StopFailure),
            "permission_denied" => Some(Self::PermissionDenied),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopNotificationSettings {
    pub enabled: bool,
    pub events: HashSet<DesktopNotificationEvent>,
}

impl Default for DesktopNotificationSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            events: DesktopNotificationEvent::DEFAULT.iter().copied().collect(),
        }
    }
}

impl DesktopNotificationSettings {
    pub fn from_tmux_options(opts: &HashMap<String, String>) -> Self {
        Self::from_tmux_options_with_backend(opts, notification_backend_available())
    }

    fn from_tmux_options_with_backend(
        opts: &HashMap<String, String>,
        backend_available: bool,
    ) -> Self {
        let enabled = read_bool(opts, tmux::SIDEBAR_NOTIFICATIONS).unwrap_or(true);
        let enabled = enabled && backend_available;
        let events = opts
            .get(tmux::SIDEBAR_NOTIFICATIONS_EVENTS)
            .map_or_else(|| Self::default().events, |raw| parse_events(raw));

        Self { enabled, events }
    }

    pub fn from_tmux() -> Self {
        Self::from_tmux_options(&tmux::get_all_global_options())
    }

    pub fn event_enabled(&self, event: DesktopNotificationEvent) -> bool {
        self.events.contains(&event)
    }
}

fn parse_events(raw: &str) -> HashSet<DesktopNotificationEvent> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return HashSet::new();
    }
    if trimmed.eq_ignore_ascii_case("all") {
        return DesktopNotificationEvent::ALL.iter().copied().collect();
    }
    trimmed
        .split(',')
        .filter_map(DesktopNotificationEvent::from_token)
        .collect()
}

pub fn format_title(repo: Option<&str>, branch: Option<&str>, agent: &str) -> String {
    let repo = repo.map(str::trim).filter(|s| !s.is_empty());
    let branch = branch.map(str::trim).filter(|s| !s.is_empty());
    match (repo, branch) {
        (Some(repo), Some(branch)) => format!("{repo} ({branch}) / {agent}"),
        (Some(repo), None) => format!("{repo} / {agent}"),
        _ => agent.to_string(),
    }
}

pub fn run_scoped_fingerprint(started_at: Option<u64>, fingerprint: &str) -> String {
    match started_at {
        Some(started_at) => format!("{started_at}:{fingerprint}"),
        None => fingerprint.to_string(),
    }
}

/// Returns true if a notification of `kind` has already fired for the
/// current `run_id` on this pane. Use to dedupe events that share a kind
/// but use distinct fingerprints (e.g. `Stop` vs explicit `TaskCompleted`
/// in the same run).
pub fn has_run_scoped_stamp(
    pane_id: &str,
    kind: DesktopNotificationKind,
    run_id: Option<u64>,
) -> bool {
    let Some(run_id) = run_id else { return false };
    if pane_id.is_empty() {
        return false;
    }
    let raw = tmux::get_pane_option_value(pane_id, stamp_option_key(kind));
    let Some(stamp) = parse_stamp(&raw) else {
        return false;
    };
    stamp.fingerprint.starts_with(&format!("{run_id}:"))
}

pub fn notify_if_allowed(
    settings: &DesktopNotificationSettings,
    pane_id: &str,
    kind: DesktopNotificationKind,
    event: DesktopNotificationEvent,
    fingerprint: &str,
    title: &str,
    body: &str,
) -> bool {
    if !settings.enabled || pane_id.is_empty() || !settings.event_enabled(event) {
        return false;
    }
    // A desktop alert is useful only when its source pane is out of view.
    if source_pane_is_visible(pane_id) {
        return false;
    }

    let key = stamp_option_key(kind);
    let normalized_fingerprint = normalize_fingerprint(fingerprint);
    let now = now_epoch_secs();
    let current = tmux::get_pane_option_value(pane_id, key);
    if let Some(stamp) = parse_stamp(&current)
        && stamp.fingerprint == normalized_fingerprint
        && now.saturating_sub(stamp.timestamp) < DESKTOP_NOTIFICATION_COOLDOWN_SECS
    {
        return false;
    }

    match send_desktop_notification(kind, title, body) {
        Ok(()) => {
            tmux::set_pane_option(pane_id, key, &encode_stamp(now, &normalized_fingerprint));
            true
        }
        Err(err) => {
            eprintln!("desktop notification failed: {err}");
            false
        }
    }
}

fn read_bool(opts: &HashMap<String, String>, key: &str) -> Option<bool> {
    let raw = opts.get(key)?.trim().to_ascii_lowercase();
    match raw.as_str() {
        "on" => Some(true),
        "off" => Some(false),
        _ => None,
    }
}

struct NotificationStamp {
    timestamp: u64,
    fingerprint: String,
}

fn stamp_option_key(kind: DesktopNotificationKind) -> &'static str {
    match kind {
        DesktopNotificationKind::TaskCompleted => tmux::PANE_OS_NOTIFY_TASK_COMPLETED,
        DesktopNotificationKind::TaskFailed => tmux::PANE_OS_NOTIFY_TASK_FAILED,
        DesktopNotificationKind::PermissionRequired => tmux::PANE_OS_NOTIFY_PERMISSION_REQUIRED,
    }
}

fn encode_stamp(timestamp: u64, fingerprint: &str) -> String {
    format!("{}|{}", timestamp, fingerprint)
}

fn parse_stamp(raw: &str) -> Option<NotificationStamp> {
    let (ts, fingerprint) = raw.split_once('|')?;
    Some(NotificationStamp {
        timestamp: ts.parse().ok()?,
        fingerprint: fingerprint.to_string(),
    })
}

fn normalize_fingerprint(value: &str) -> String {
    value.replace(['|', '\n', '\r'], " ")
}

#[cfg(target_os = "linux")]
fn source_pane_is_visible(pane_id: &str) -> bool {
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_none() {
        return false;
    }
    let output = |args: &[&str]| -> Option<String> {
        let result = Command::new("timeout")
            .arg("0.5s")
            .args(args)
            .stderr(Stdio::null())
            .output()
            .ok()?;
        result
            .status
            .success()
            .then(|| String::from_utf8_lossy(&result.stdout).into_owned())
    };
    let Some(target) = output(&[
        "tmux",
        "display-message",
        "-p",
        "-t",
        pane_id,
        "#{session_name}|#{window_id}",
    ]) else {
        return false;
    };
    let Some(clients) = output(&[
        "tmux",
        "list-clients",
        "-F",
        "#{client_pid}|#{client_session}|#{window_id}",
    ]) else {
        return false;
    };
    let Some(windows) = output(&["hyprctl", "clients", "-j"])
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
    else {
        return false;
    };
    let Some(monitors) = output(&["hyprctl", "monitors", "-j"])
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
    else {
        return false;
    };
    let visible_workspaces: HashSet<i64> = monitors
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|m| {
            [
                m["activeWorkspace"]["id"].as_i64(),
                m["specialWorkspace"]["id"].as_i64(),
            ]
        })
        .flatten()
        .filter(|id| *id != 0)
        .collect();
    for client in clients.lines() {
        let Some((pid, location)) = client.split_once('|') else {
            continue;
        };
        if location != target.trim() {
            continue;
        }
        let Ok(mut pid) = pid.parse::<u32>() else {
            continue;
        };
        let mut ancestors = HashSet::new();
        for _ in 0..64 {
            if pid <= 1 || !ancestors.insert(pid) {
                break;
            }
            let Some(parent) = std::fs::read_to_string(format!("/proc/{pid}/status"))
                .ok()
                .and_then(|status| {
                    status.lines().find_map(|line| {
                        line.strip_prefix("PPid:")
                            .and_then(|s| s.trim().parse::<u32>().ok())
                    })
                })
            else {
                break;
            };
            pid = parent;
        }
        if windows
            .as_array()
            .into_iter()
            .flatten()
            .any(|window| window_is_visible(window, &ancestors, &visible_workspaces))
        {
            return true;
        }
    }
    false
}

#[cfg(target_os = "linux")]
fn window_is_visible(
    window: &serde_json::Value,
    ancestors: &HashSet<u32>,
    visible_workspaces: &HashSet<i64>,
) -> bool {
    window["mapped"].as_bool() == Some(true)
        && window["hidden"].as_bool() != Some(true)
        && window["pid"]
            .as_u64()
            .is_some_and(|pid| ancestors.contains(&(pid as u32)))
        && window["workspace"]["id"]
            .as_i64()
            .is_some_and(|id| visible_workspaces.contains(&id))
}

#[cfg(not(target_os = "linux"))]
fn source_pane_is_visible(_pane_id: &str) -> bool {
    false
}

// Bound every notification, including error details and long branch names.
fn compact_notification_text(text: &str, max_chars: usize) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let text: String = text.chars().filter(|c| !c.is_control()).collect();
    if text.chars().count() <= max_chars {
        text
    } else {
        let mut result: String = text.chars().take(max_chars.saturating_sub(1)).collect();
        result.push('…');
        result
    }
}

fn notification_label(kind: DesktopNotificationKind) -> &'static str {
    match kind {
        DesktopNotificationKind::TaskCompleted => "✓ Done",
        DesktopNotificationKind::TaskFailed => "✕ Failed",
        DesktopNotificationKind::PermissionRequired => "Needs attention",
    }
}

#[cfg(target_os = "linux")]
fn notification_icon(kind: DesktopNotificationKind) -> String {
    let (file, fallback) = match kind {
        DesktopNotificationKind::TaskCompleted => {
            ("emblems/vcs-normal.svg", "emblem-rabbitvcs-normal")
        }
        DesktopNotificationKind::TaskFailed => ("status/dialog-error.svg", "dialog-error"),
        DesktopNotificationKind::PermissionRequired => {
            ("status/dialog-question.svg", "dialog-question")
        }
    };
    // Papirus-Dark inherits these status icons from Papirus. Absolute paths
    // also work when the notification daemon cannot resolve the icon theme.
    let path = std::path::Path::new("/usr/share/icons/Papirus/48x48").join(file);
    if path.is_file() {
        return path.to_string_lossy().into_owned();
    }
    fallback.to_string()
}

fn send_desktop_notification(
    kind: DesktopNotificationKind,
    title: &str,
    body: &str,
) -> Result<(), String> {
    let title = format!(
        "{} · {}",
        notification_label(kind),
        compact_notification_text(title, 64)
    );
    let body = compact_notification_text(body, 120);
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "display notification \"{}\" with title \"{}\"",
            escape_applescript(&body),
            escape_applescript(&title)
        );
        let mut command = Command::new("osascript");
        command
            .args(["-e", &script])
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        run_notification_command(&mut command, "osascript", DESKTOP_NOTIFICATION_TIMEOUT)
    }

    #[cfg(target_os = "linux")]
    {
        // SwayNC interprets body markup. Treat agent-supplied details as text.
        let body = body
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        let icon = notification_icon(kind);
        let mut command = Command::new("notify-send");
        command
            .args([
                "--app-name=agents-tmux",
                "--urgency=normal",
                "--expire-time=6000",
                "--icon",
                &icon,
                "--",
                &title,
                &body,
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        run_notification_command(&mut command, "notify-send", DESKTOP_NOTIFICATION_TIMEOUT)
    }

    #[cfg(target_os = "windows")]
    {
        let _ = (title, body);
        Err("desktop notifications are not supported on Windows yet".into())
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = (title, body);
        Err("desktop notifications are not supported on this platform".into())
    }
}

fn notification_backend_available() -> bool {
    #[cfg(target_os = "macos")]
    {
        let mut command = Command::new("osascript");
        command
            .args(["-e", "return 0"])
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        run_notification_command(
            &mut command,
            "osascript",
            DESKTOP_NOTIFICATION_PROBE_TIMEOUT,
        )
        .is_ok()
    }

    #[cfg(target_os = "linux")]
    {
        let mut command = Command::new("notify-send");
        command
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        run_notification_command(
            &mut command,
            "notify-send",
            DESKTOP_NOTIFICATION_PROBE_TIMEOUT,
        )
        .is_ok()
    }

    #[cfg(target_os = "windows")]
    {
        false
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        false
    }
}

#[cfg(target_os = "macos")]
fn escape_applescript(value: &str) -> String {
    value
        .replace(['\n', '\r'], " ")
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}

fn run_notification_command(
    command: &mut Command,
    command_name: &str,
    timeout: Duration,
) -> Result<(), String> {
    let mut child = command
        .spawn()
        .map_err(|err| format!("failed to spawn {command_name}: {err}"))?;
    let start = Instant::now();

    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => {
                return Err(format!("{command_name} exited with status {status}"));
            }
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "{command_name} timed out after {}s",
                        timeout.as_secs()
                    ));
                }
                sleep(Duration::from_millis(25));
            }
            Err(err) => return Err(format!("failed to wait on {command_name}: {err}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn notifications_are_silent_only_for_a_visible_source_terminal() {
        let ancestors = HashSet::from([42, 100]);
        let workspaces = HashSet::from([1, 6]);
        let mut window =
            serde_json::json!({"pid":42,"mapped":true,"hidden":false,"workspace":{"id":1}});
        assert!(window_is_visible(&window, &ancestors, &workspaces));
        window["workspace"]["id"] = 3.into();
        assert!(!window_is_visible(&window, &ancestors, &workspaces));
        window["workspace"]["id"] = 1.into();
        window["pid"] = 777.into();
        assert!(!window_is_visible(&window, &ancestors, &workspaces));
        window["pid"] = 42.into();
        window["hidden"] = true.into();
        assert!(!window_is_visible(&window, &ancestors, &workspaces));
    }

    #[test]
    fn notification_text_is_short_single_line_and_unicode_safe() {
        assert_eq!(
            compact_notification_text("  hello\n\tworld\r\n", 20),
            "hello world"
        );
        assert_eq!(compact_notification_text("é日本語abcdef", 5), "é日本語…");
        assert_eq!(
            compact_notification_text(&"x".repeat(10_000), 120)
                .chars()
                .count(),
            120
        );
        assert_eq!(compact_notification_text("hi\0there", 20), "hithere");
    }

    #[test]
    fn notification_kinds_have_distinct_labels() {
        assert_eq!(
            notification_label(DesktopNotificationKind::TaskCompleted),
            "✓ Done"
        );
        assert_eq!(
            notification_label(DesktopNotificationKind::TaskFailed),
            "✕ Failed"
        );
        assert_eq!(
            notification_label(DesktopNotificationKind::PermissionRequired),
            "Needs attention"
        );
    }

    #[test]
    fn settings_parse_bool_and_numbers() {
        let mut opts = HashMap::new();
        opts.insert(tmux::SIDEBAR_NOTIFICATIONS.into(), "on".into());

        let settings = DesktopNotificationSettings::from_tmux_options_with_backend(&opts, true);
        assert!(settings.enabled);
    }

    #[test]
    fn settings_default_when_invalid() {
        let mut opts = HashMap::new();
        opts.insert(tmux::SIDEBAR_NOTIFICATIONS.into(), "maybe".into());

        let settings = DesktopNotificationSettings::from_tmux_options_with_backend(&opts, true);
        assert!(settings.enabled);
    }

    #[test]
    fn settings_disable_when_off() {
        let mut opts = HashMap::new();
        opts.insert(tmux::SIDEBAR_NOTIFICATIONS.into(), "off".into());

        let settings = DesktopNotificationSettings::from_tmux_options_with_backend(&opts, true);
        assert!(!settings.enabled);
    }

    #[test]
    fn settings_disable_when_backend_missing() {
        let opts = HashMap::new();
        let settings = DesktopNotificationSettings::from_tmux_options_with_backend(&opts, false);
        assert!(!settings.enabled);
    }

    #[test]
    fn format_title_variants() {
        assert_eq!(
            format_title(Some("repo"), Some("feat/xyz"), "claude"),
            "repo (feat/xyz) / claude"
        );
        assert_eq!(format_title(Some("repo"), None, "claude"), "repo / claude");
        assert_eq!(
            format_title(Some("repo"), Some(""), "claude"),
            "repo / claude"
        );
        assert_eq!(format_title(None, Some("feat"), "claude"), "claude");
        assert_eq!(format_title(None, None, "claude"), "claude");
    }

    #[test]
    fn stamp_round_trip() {
        let stamp = encode_stamp(123, "foo bar");
        let parsed = parse_stamp(&stamp).unwrap();
        assert_eq!(parsed.timestamp, 123);
        assert_eq!(parsed.fingerprint, "foo bar");
    }

    #[test]
    fn fingerprint_is_normalized() {
        assert_eq!(
            normalize_fingerprint("foo|bar\nbaz\rqux"),
            "foo bar baz qux"
        );
    }

    #[test]
    fn events_default_to_default_set_when_unset() {
        let opts = HashMap::new();
        let settings = DesktopNotificationSettings::from_tmux_options_with_backend(&opts, true);
        for event in DesktopNotificationEvent::DEFAULT {
            assert!(settings.event_enabled(event), "expected {event:?} enabled");
        }
        assert!(
            !settings.event_enabled(DesktopNotificationEvent::TaskCompleted),
            "task_completed should be opt-in"
        );
    }

    #[test]
    fn events_parse_explicit_subset() {
        let mut opts = HashMap::new();
        opts.insert(
            tmux::SIDEBAR_NOTIFICATIONS_EVENTS.into(),
            "stop, permission_denied".into(),
        );
        let settings = DesktopNotificationSettings::from_tmux_options_with_backend(&opts, true);
        assert!(settings.event_enabled(DesktopNotificationEvent::Stop));
        assert!(settings.event_enabled(DesktopNotificationEvent::PermissionDenied));
        assert!(!settings.event_enabled(DesktopNotificationEvent::Notification));
        assert!(!settings.event_enabled(DesktopNotificationEvent::TaskCompleted));
        assert!(!settings.event_enabled(DesktopNotificationEvent::StopFailure));
    }

    #[test]
    fn events_all_keyword_enables_every_event() {
        let mut opts = HashMap::new();
        opts.insert(tmux::SIDEBAR_NOTIFICATIONS_EVENTS.into(), "all".into());
        let settings = DesktopNotificationSettings::from_tmux_options_with_backend(&opts, true);
        for event in DesktopNotificationEvent::ALL {
            assert!(settings.event_enabled(event));
        }
    }

    #[test]
    fn events_empty_value_disables_every_event() {
        let mut opts = HashMap::new();
        opts.insert(tmux::SIDEBAR_NOTIFICATIONS_EVENTS.into(), "".into());
        let settings = DesktopNotificationSettings::from_tmux_options_with_backend(&opts, true);
        for event in DesktopNotificationEvent::ALL {
            assert!(!settings.event_enabled(event));
        }
    }

    #[test]
    fn events_unknown_tokens_are_ignored() {
        let mut opts = HashMap::new();
        opts.insert(
            tmux::SIDEBAR_NOTIFICATIONS_EVENTS.into(),
            "stop,bogus, task_completed".into(),
        );
        let settings = DesktopNotificationSettings::from_tmux_options_with_backend(&opts, true);
        assert!(settings.event_enabled(DesktopNotificationEvent::Stop));
        assert!(settings.event_enabled(DesktopNotificationEvent::TaskCompleted));
        assert!(!settings.event_enabled(DesktopNotificationEvent::Notification));
    }

    #[test]
    fn has_run_scoped_stamp_returns_false_without_stamp() {
        let _guard = tmux::test_mock::install();
        let pane = "%PANE_NO_STAMP";
        assert!(!has_run_scoped_stamp(
            pane,
            DesktopNotificationKind::TaskCompleted,
            Some(1_700_000_000_000),
        ));
    }

    #[test]
    fn has_run_scoped_stamp_matches_current_run() {
        let _guard = tmux::test_mock::install();
        let pane = "%PANE_CURRENT_RUN";
        let run_id = 1_700_000_000_000_u64;
        let stamp = encode_stamp(42, &format!("{run_id}:task-xyz"));
        tmux::test_mock::set(
            pane,
            stamp_option_key(DesktopNotificationKind::TaskCompleted),
            &stamp,
        );
        assert!(has_run_scoped_stamp(
            pane,
            DesktopNotificationKind::TaskCompleted,
            Some(run_id),
        ));
    }

    #[test]
    fn has_run_scoped_stamp_rejects_stale_run() {
        let _guard = tmux::test_mock::install();
        let pane = "%PANE_STALE_RUN";
        let old_run = 1_600_000_000_000_u64;
        let new_run = 1_700_000_000_000_u64;
        let stamp = encode_stamp(42, &format!("{old_run}:task-xyz"));
        tmux::test_mock::set(
            pane,
            stamp_option_key(DesktopNotificationKind::TaskCompleted),
            &stamp,
        );
        assert!(!has_run_scoped_stamp(
            pane,
            DesktopNotificationKind::TaskCompleted,
            Some(new_run),
        ));
    }

    #[test]
    fn has_run_scoped_stamp_requires_run_id() {
        let _guard = tmux::test_mock::install();
        let pane = "%PANE_NO_RUN_ID";
        let stamp = encode_stamp(42, "1700000000000:task-xyz");
        tmux::test_mock::set(
            pane,
            stamp_option_key(DesktopNotificationKind::TaskCompleted),
            &stamp,
        );
        assert!(!has_run_scoped_stamp(
            pane,
            DesktopNotificationKind::TaskCompleted,
            None,
        ));
    }
}
