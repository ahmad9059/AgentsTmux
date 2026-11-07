//! Subscription usage for the focused agent.
//!
//! The sidebar's bottom panel shows what is actually *available* for the
//! agent the user is looking at right now:
//!
//! * Claude Code → the Anthropic OAuth usage endpoint (`five_hour` session
//!   and `seven_day` weekly windows) plus any extra-usage credits.
//! * Codex → the ChatGPT backend `wham/usage` endpoint (primary 5-hour
//!   window, secondary weekly window, plan and credits), falling back to the
//!   `rate_limits` snapshot Codex itself writes into its rollout files.
//! * OpenCode / Pi → they are harnesses rather than subscriptions, so we
//!   report the local session spend from their own data stores, and reuse
//!   the Claude or Codex windows when the harness is running on one of
//!   those subscriptions.
//!
//! Network access goes through `curl` (the same transport the version
//! checker uses) so the binary stays dependency-free, and the `--max-time`
//! caps keep a slow endpoint from blocking the TUI — every call happens on
//! a background thread.

use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::time::now_epoch_secs;
use crate::tmux::AgentType;

mod http;
mod native;

pub const CLAUDE_USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
pub const CODEX_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";

/// How long a provider fetch is reused before going back to the network.
/// Matches the visible-tab refresh interval, so a panel you are looking at
/// really does re-check the provider every tick; it still stops two agents on
/// the same account from double-fetching when you switch between them.
pub const PROVIDER_CACHE_TTL: Duration = Duration::from_secs(10);

/// One rate-limit window (5-hour session, weekly, …).
#[derive(Debug, Clone, PartialEq)]
pub struct UsageWindow {
    /// Short left-hand label, e.g. `5h` or `1w`.
    pub label: String,
    /// Percent used, 0–100.
    pub percent: f32,
    /// Unix epoch seconds when the window resets, when the provider says.
    pub resets_at: Option<u64>,
}

/// Everything the Usage tab renders for one focused agent.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct UsageSnapshot {
    /// Identifies the pane/agent combination this snapshot belongs to.
    pub key: String,
    /// Provider heading, e.g. `Claude Code`.
    pub title: String,
    /// Plan name when the provider reports one (`team`, `max`, …).
    pub plan: Option<String>,
    /// Rate-limit windows, empty when the provider has no limits to report.
    pub windows: Vec<UsageWindow>,
    /// Extra credit/spend lines shown under the bars.
    pub extras: Vec<String>,
    /// Local session spend lines (OpenCode, Pi).
    pub local: Vec<(String, String)>,
    /// Explanation when there is nothing to meter.
    pub message: Option<String>,
    pub fetched_at: u64,
}

/// The agent the Usage tab should describe.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageTarget {
    /// Changes whenever the focused pane, agent, or session changes.
    pub key: String,
    pub agent: AgentType,
    /// Pi reports itself through the Claude adapter, so it needs a marker.
    pub is_pi: bool,
    pub session_id: Option<String>,
    pub cwd: String,
}

/// Provider data shared by every agent that runs on the same subscription.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderUsage {
    pub plan: Option<String>,
    pub windows: Vec<UsageWindow>,
    pub extras: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocalUsage {
    pub provider: String,
    pub model: String,
    pub session_cost: Option<f64>,
    pub session_tokens: Option<u64>,
    pub today_cost: Option<f64>,
}

/// Fetches usage for one target, caching the (account-wide) provider
/// windows so switching between two Claude panes does not re-hit the API.
#[derive(Debug, Default)]
pub struct UsageFetcher {
    claude: Option<(Instant, Result<ProviderUsage, String>)>,
    codex: Option<(Instant, Result<ProviderUsage, String>)>,
}

impl UsageFetcher {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn fetch(&mut self, target: &UsageTarget) -> UsageSnapshot {
        let fetched_at = now_epoch_secs();
        let mut snapshot = UsageSnapshot {
            key: target.key.clone(),
            fetched_at,
            ..UsageSnapshot::default()
        };

        match target.agent {
            AgentType::Claude if !target.is_pi => {
                snapshot.title = "Claude Code".into();
                snapshot.local = native::details(target, fetched_at);
                match self.claude_cached() {
                    Ok(usage) => apply_provider(&mut snapshot, usage),
                    Err(err) => snapshot.message = Some(err),
                }
            }
            AgentType::Codex => {
                snapshot.title = "Codex".into();
                snapshot.local = native::details(target, fetched_at);
                match self.codex_cached() {
                    Ok(usage) => apply_provider(&mut snapshot, usage),
                    Err(err) => snapshot.message = Some(err),
                }
            }
            AgentType::OpenCode => {
                snapshot.title = "OpenCode".into();
                let local = opencode_local_usage(target);
                match local {
                    Ok(local) => {
                        snapshot.plan = None;
                        snapshot.local = local_lines(&local);
                        match provider_for(&local.provider) {
                            Some(ProviderKind::Claude) => {
                                self.attach_provider(&mut snapshot, true, target)
                            }
                            Some(ProviderKind::Codex) => {
                                self.attach_provider(&mut snapshot, false, target)
                            }
                            None => {
                                snapshot.message =
                                    Some(format!("No limits for {}", local.provider));
                            }
                        }
                    }
                    Err(err) => snapshot.message = Some(err),
                }
            }
            AgentType::Claude => {
                // Pi goes through the Claude adapter; read Pi's own stores.
                snapshot.title = "Pi".into();
                match pi_local_usage(target) {
                    Ok(local) => {
                        snapshot.local = local_lines(&local);
                        match provider_for(&local.provider) {
                            Some(ProviderKind::Claude) => {
                                self.attach_provider(&mut snapshot, true, target)
                            }
                            Some(ProviderKind::Codex) => {
                                self.attach_provider(&mut snapshot, false, target)
                            }
                            None => {
                                snapshot.message =
                                    Some(format!("No limits for {}", local.provider));
                            }
                        }
                    }
                    Err(err) => snapshot.message = Some(err),
                }
            }
            AgentType::Unknown => {
                snapshot.title = "Usage".into();
                snapshot.message = Some("No usage for this agent".into());
            }
        }

        snapshot
    }

    fn attach_provider(
        &mut self,
        snapshot: &mut UsageSnapshot,
        claude: bool,
        target: &UsageTarget,
    ) {
        let usage = harness_credentials(target, claude).and_then(|credential| {
            if claude {
                fetch_claude_with_token(&credential.0, None)
            } else {
                fetch_codex_with_token(&credential.0, &credential.1)
            }
        });
        match usage {
            Ok(usage) => apply_provider(snapshot, usage),
            Err(err) => {
                if snapshot.message.is_none() {
                    snapshot.message = Some(err);
                }
            }
        }
    }

    fn claude_cached(&mut self) -> Result<ProviderUsage, String> {
        if let Some((at, ref data)) = self.claude
            && at.elapsed() < PROVIDER_CACHE_TTL
        {
            return data.clone();
        }
        let data = fetch_claude_usage();
        self.claude = Some((Instant::now(), data.clone()));
        data
    }

    fn codex_cached(&mut self) -> Result<ProviderUsage, String> {
        if let Some((at, ref data)) = self.codex
            && at.elapsed() < PROVIDER_CACHE_TTL
        {
            return data.clone();
        }
        let data = fetch_codex_usage();
        self.codex = Some((Instant::now(), data.clone()));
        data
    }
}

fn apply_provider(snapshot: &mut UsageSnapshot, usage: ProviderUsage) {
    if let Some(plan) = usage.plan {
        snapshot.plan = Some(plan);
    }
    snapshot.windows = usage.windows;
    snapshot.extras.extend(usage.extras);
}

/// Providers whose subscription windows we can query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderKind {
    Claude,
    Codex,
}

fn provider_for(provider: &str) -> Option<ProviderKind> {
    let provider = provider.to_ascii_lowercase();
    if provider.contains("anthropic") || provider.contains("claude") {
        Some(ProviderKind::Claude)
    } else if provider.contains("openai") || provider.contains("codex") {
        Some(ProviderKind::Codex)
    } else {
        None
    }
}

fn local_lines(local: &LocalUsage) -> Vec<(String, String)> {
    let mut lines = Vec::new();
    let mut provider = local.provider.clone();
    if provider.is_empty() {
        provider = local.model.clone();
    }
    if !provider.is_empty() {
        lines.push(("Using".into(), provider));
    }
    // Keep the model on its own row: provider names are long enough that
    // `provider/model` gets truncated in a narrow sidebar.
    if !local.model.is_empty() && !local.provider.is_empty() {
        lines.push(("Model".into(), local.model.clone()));
    }
    if let Some(cost) = local.session_cost {
        let tokens = local
            .session_tokens
            .map(|t| format!(" · {}", format_tokens(t)))
            .unwrap_or_default();
        lines.push(("Session".into(), format!("${cost:.2}{tokens}")));
    } else if let Some(tokens) = local.session_tokens {
        lines.push(("Session".into(), format_tokens(tokens)));
    }
    if let Some(cost) = local.today_cost {
        lines.push(("Today".into(), format!("${cost:.2}")));
    }
    lines
}

// ─── Claude ─────────────────────────────────────────────────────────

fn fetch_claude_usage() -> Result<ProviderUsage, String> {
    let (token, plan) = claude_credentials()?;
    match fetch_claude_with_token(&token, plan.clone()) {
        Ok(usage) => Ok(usage),
        Err(original_error) => {
            // An OAuth application's usage endpoint can be throttled while
            // another application on the exact same account still works.
            // Only reuse its quota after the provider verifies BOTH account
            // and organization identity. Never borrow an unrelated account.
            let target = UsageTarget {
                key: String::new(),
                agent: AgentType::OpenCode,
                is_pi: false,
                session_id: None,
                cwd: String::new(),
            };
            let fallback = harness_credentials(&target, true).and_then(|(other, _)| {
                if token == other || !verified_same_claude_account(&token, &other) {
                    return Err(original_error.clone());
                }
                fetch_claude_with_token(&other, plan)
            });
            fallback.map_err(|_| original_error)
        }
    }
}

fn verified_same_claude_account(first: &str, second: &str) -> bool {
    let identity = |token: &str| -> Option<(String, String)> {
        let profile = http_get_json(
            "https://api.anthropic.com/api/oauth/profile",
            &[
                ("Authorization", format!("Bearer {token}")),
                ("anthropic-beta", "oauth-2025-04-20".into()),
                ("User-Agent", "agents-tmux".into()),
            ],
        )
        .ok()?;
        claude_account_identity(&profile)
    };
    match (identity(first), identity(second)) {
        (Some(first), Some(second)) => first == second,
        _ => false,
    }
}

fn claude_account_identity(profile: &Value) -> Option<(String, String)> {
    let account = profile.pointer("/account/uuid")?.as_str()?.trim();
    let organization = profile.pointer("/organization/uuid")?.as_str()?.trim();
    if account.is_empty() || organization.is_empty() {
        return None;
    }
    Some((account.into(), organization.into()))
}

fn fetch_claude_with_token(token: &str, plan: Option<String>) -> Result<ProviderUsage, String> {
    let headers = [
        ("Authorization", format!("Bearer {token}")),
        ("anthropic-beta", "oauth-2025-04-20".to_string()),
        ("User-Agent", "agents-tmux".to_string()),
    ];
    let value = http_get_json(CLAUDE_USAGE_URL, &headers)?;
    parse_claude_usage(&value, plan)
}

fn parse_claude_usage(value: &Value, plan: Option<String>) -> Result<ProviderUsage, String> {
    let mut windows = Vec::new();
    for (path, label) in [("/five_hour", "5h"), ("/seven_day", "1w")] {
        let Some(window) = value.pointer(path) else {
            continue;
        };
        let Some(percent) = window.get("utilization").and_then(Value::as_f64) else {
            continue;
        };
        windows.push(UsageWindow {
            label: label.to_string(),
            percent: percent as f32,
            resets_at: window
                .get("resets_at")
                .and_then(Value::as_str)
                .and_then(parse_rfc3339),
        });
    }
    if windows.is_empty() {
        return Err("Claude usage unavailable".into());
    }

    let mut extras = Vec::new();
    stale_notice(value, &mut extras);
    if let Some(extra) = value.get("extra_usage")
        && extra
            .get("is_enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    {
        let used = extra
            .get("used_credits")
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
            / 100.0;
        let limit = extra
            .get("monthly_limit")
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
            / 100.0;
        extras.push(format!("Extra ${used:.2} / ${limit:.2}"));
    }

    Ok(ProviderUsage {
        plan,
        windows,
        extras,
    })
}

fn claude_credentials() -> Result<(String, Option<String>), String> {
    let path = config_home(".claude").join(".credentials.json");
    let raw = fs::read_to_string(&path)
        .map_err(|_| "No Claude credentials — sign in with `claude`".to_string())?;
    let value: Value =
        serde_json::from_str(&raw).map_err(|_| "Claude credentials unreadable".to_string())?;
    let oauth = value.get("claudeAiOauth");
    let token = oauth
        .and_then(|o| o.get("accessToken"))
        .and_then(Value::as_str)
        .ok_or_else(|| "No Claude token in credentials".to_string())?;
    let plan = oauth
        .and_then(|o| o.get("subscriptionType"))
        .and_then(Value::as_str)
        .map(str::to_string);
    Ok((token.to_string(), plan))
}

// ─── Codex ──────────────────────────────────────────────────────────

fn fetch_codex_usage() -> Result<ProviderUsage, String> {
    let (token, account) = codex_credentials()?;
    fetch_codex_with_token(&token, &account)
        .or_else(|network_error| codex_usage_from_rollout().map_err(|_| network_error))
}

fn fetch_codex_with_token(token: &str, account: &str) -> Result<ProviderUsage, String> {
    let headers = [
        ("Authorization", format!("Bearer {token}")),
        ("chatgpt-account-id", account.to_string()),
        ("User-Agent", "agents-tmux".to_string()),
    ];
    parse_codex_usage(&http_get_json(CODEX_USAGE_URL, &headers)?)
}

fn codex_credentials() -> Result<(String, String), String> {
    let path = config_home(".codex").join("auth.json");
    let raw = fs::read_to_string(&path)
        .map_err(|_| "No Codex credentials — sign in with `codex`".to_string())?;
    let value: Value =
        serde_json::from_str(&raw).map_err(|_| "Codex credentials unreadable".to_string())?;
    let tokens = value
        .get("tokens")
        .ok_or_else(|| "Codex is signed in with an API key".to_string())?;
    let token = tokens
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or_else(|| "No Codex access token".to_string())?;
    let account = tokens
        .get("account_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    Ok((token.to_string(), account))
}

fn parse_codex_usage(value: &Value) -> Result<ProviderUsage, String> {
    let rate_limit = value
        .get("rate_limit")
        .ok_or_else(|| "Codex usage unavailable".to_string())?;
    let mut windows = Vec::new();
    for (key, fallback_label) in [("primary_window", "5h"), ("secondary_window", "1w")] {
        let Some(window) = rate_limit.get(key) else {
            continue;
        };
        let Some(percent) = window.get("used_percent").and_then(Value::as_f64) else {
            continue;
        };
        let label = window
            .get("limit_window_seconds")
            .and_then(Value::as_u64)
            .map(window_label)
            .unwrap_or_else(|| fallback_label.to_string());
        windows.push(UsageWindow {
            label,
            percent: percent as f32,
            resets_at: window.get("reset_at").and_then(Value::as_u64),
        });
    }
    if windows.is_empty() {
        return Err("Codex usage unavailable".into());
    }

    let mut extras = Vec::new();
    stale_notice(value, &mut extras);
    if rate_limit
        .get("limit_reached")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        extras.push("Limit reached".to_string());
    }
    if let Some(credits) = value.get("credits")
        && credits.get("has_credits").and_then(Value::as_bool) == Some(true)
        && credits.get("unlimited").and_then(Value::as_bool) == Some(false)
        && let Some(balance) = credits.get("balance").and_then(Value::as_f64)
    {
        extras.push(format!("Credits {balance:.2}"));
    }

    Ok(ProviderUsage {
        plan: value
            .get("plan_type")
            .and_then(Value::as_str)
            .map(str::to_string),
        windows,
        extras,
    })
}

/// Read the last `rate_limits` snapshot Codex wrote into its rollout file.
/// Used when the network call fails so the tab still shows the last numbers
/// instead of an error.
fn codex_usage_from_rollout() -> Result<ProviderUsage, String> {
    let file = newest_rollout_file(&config_home(".codex").join("sessions"))
        .ok_or_else(|| "Codex usage unavailable".to_string())?;
    let tail = read_tail(&file, 256 * 1024).ok_or_else(|| "Codex usage unavailable".to_string())?;
    for line in tail.lines().rev() {
        if !line.contains("\"rate_limits\"") {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(limits) = value.pointer("/payload/rate_limits") else {
            continue;
        };
        let mut windows = Vec::new();
        for (key, fallback_label) in [("primary", "5h"), ("secondary", "1w")] {
            let Some(window) = limits.get(key) else {
                continue;
            };
            let Some(percent) = window.get("used_percent").and_then(Value::as_f64) else {
                continue;
            };
            let label = window
                .get("window_minutes")
                .and_then(Value::as_u64)
                .map(|minutes| window_label(minutes * 60))
                .unwrap_or_else(|| fallback_label.to_string());
            windows.push(UsageWindow {
                label,
                percent: percent as f32,
                resets_at: window.get("resets_at").and_then(Value::as_u64),
            });
        }
        if windows.is_empty() {
            continue;
        }
        return Ok(ProviderUsage {
            plan: limits
                .get("plan_type")
                .and_then(Value::as_str)
                .map(str::to_string),
            windows,
            extras: vec!["Last Codex session".to_string()],
        });
    }
    Err("Codex usage unavailable".into())
}

fn newest_rollout_file(root: &Path) -> Option<PathBuf> {
    fn walk(dir: &Path, depth: usize, best: &mut Option<(std::time::SystemTime, PathBuf)>) {
        if depth > 5 {
            return;
        }
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                walk(&path, depth + 1, best);
            } else if path.extension().is_some_and(|ext| ext == "jsonl")
                && let Ok(modified) = meta.modified()
                && best.as_ref().is_none_or(|(at, _)| modified > *at)
            {
                *best = Some((modified, path));
            }
        }
    }
    let mut best = None;
    walk(root, 0, &mut best);
    best.map(|(_, path)| path)
}

fn read_tail(path: &Path, max_bytes: u64) -> Option<String> {
    let mut file = fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(max_bytes);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = String::new();
    file.read_to_string(&mut buf).ok()?;
    if start > 0 {
        // Drop the partial first line.
        if let Some(idx) = buf.find('\n') {
            buf.drain(..=idx);
        }
    }
    Some(buf)
}

/// Short label for a rate-limit window length: `5h`, `1w`, `3d`.
fn window_label(seconds: u64) -> String {
    if seconds == 0 {
        return "—".into();
    }
    if seconds.is_multiple_of(604_800) {
        format!("{}w", seconds / 604_800)
    } else if seconds.is_multiple_of(86_400) {
        format!("{}d", seconds / 86_400)
    } else if seconds.is_multiple_of(3_600) && seconds < 86_400 {
        format!("{}h", seconds / 3_600)
    } else {
        format!("{}m", seconds / 60)
    }
}

// ─── OpenCode ───────────────────────────────────────────────────────

fn opencode_local_usage(target: &UsageTarget) -> Result<LocalUsage, String> {
    let db = config_home(".local/share").join("opencode/opencode.db");
    if !db.exists() {
        return Err("No OpenCode usage data".into());
    }
    let session_row = target
        .session_id
        .as_deref()
        .and_then(|id| sqlite3(&db, &opencode_session_sql(id)))
        .or_else(|| sqlite3(&db, &opencode_latest_sql(&target.cwd)));

    let (provider, model, cost, tokens) = match session_row {
        Some(row) => parse_opencode_row(&row),
        None => (String::new(), String::new(), None, None),
    };
    let today = sqlite3(&db, opencode_today_sql()).and_then(|row| row.trim().parse::<f64>().ok());

    Ok(LocalUsage {
        provider,
        model,
        session_cost: cost,
        session_tokens: tokens,
        today_cost: today,
    })
}

fn opencode_session_sql(session_id: &str) -> String {
    format!(
        "select coalesce(cost,0), coalesce(tokens_input,0)+coalesce(tokens_output,0), coalesce(model,'') \
         from session_v2 where id='{}' limit 1;",
        sql_escape(session_id)
    )
}

fn opencode_latest_sql(cwd: &str) -> String {
    format!(
        "select coalesce(cost,0), coalesce(tokens_input,0)+coalesce(tokens_output,0), coalesce(model,'') \
         from session_v2 where directory='{}' order by time_updated desc limit 1;",
        sql_escape(cwd)
    )
}

fn opencode_today_sql() -> &'static str {
    "select coalesce(sum(cost),0) from session_v2 \
     where time_updated >= strftime('%s', date('now','localtime')) * 1000;"
}

fn parse_opencode_row(row: &str) -> (String, String, Option<f64>, Option<u64>) {
    let mut fields = row.trim().split('|');
    let cost = fields.next().and_then(|v| v.trim().parse::<f64>().ok());
    let tokens = fields.next().and_then(|v| v.trim().parse::<u64>().ok());
    let model_raw = fields.next().unwrap_or("").trim();
    let (provider, model) = serde_json::from_str::<Value>(model_raw)
        .ok()
        .map(|value| {
            (
                value
                    .get("providerID")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                value
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .unwrap_or_default();
    (provider, model, cost, tokens)
}

fn sql_escape(value: &str) -> String {
    value.replace('\'', "''")
}

fn sqlite3(db: &Path, sql: &str) -> Option<String> {
    let output = Command::new("sqlite3")
        .args(["-readonly", "-separator", "|"])
        .arg(db)
        .arg(sql)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).to_string())
}

// ─── Pi ─────────────────────────────────────────────────────────────

fn pi_local_usage(target: &UsageTarget) -> Result<LocalUsage, String> {
    let root = config_home(".pi").join("agent");
    let settings = fs::read_to_string(root.join("settings.json")).ok();
    let default_provider = settings
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .and_then(|value| {
            value
                .get("defaultProvider")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default();

    let file = pi_session_file(&root.join("sessions"), target)
        .ok_or_else(|| "No Pi usage data".to_string())?;
    let raw = fs::read_to_string(&file).map_err(|_| "Pin usage data unreadable".to_string())?;

    let mut provider = String::new();
    let mut model = String::new();
    let mut tokens: u64 = 0;
    let mut cost: f64 = 0.0;
    let mut saw_cost = false;
    for line in raw.lines() {
        if !line.contains("\"role\":\"assistant\"") {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(message) = value.get("message") else {
            continue;
        };
        if let Some(p) = message.get("provider").and_then(Value::as_str) {
            provider = p.to_string();
        }
        if let Some(m) = message.get("model").and_then(Value::as_str) {
            model = m.to_string();
        }
        if let Some(usage) = message.get("usage") {
            tokens += usage
                .get("totalTokens")
                .and_then(Value::as_u64)
                .or_else(|| usage.get("input").and_then(Value::as_u64))
                .unwrap_or(0);
            if let Some(total) = usage.pointer("/cost/total").and_then(Value::as_f64) {
                cost += total;
                saw_cost = true;
            }
        }
    }
    if provider.is_empty() {
        provider = default_provider;
    }
    if tokens == 0 && !saw_cost {
        return Err("No Pi usage data".into());
    }

    Ok(LocalUsage {
        provider,
        model,
        session_cost: saw_cost.then_some(cost),
        session_tokens: (tokens > 0).then_some(tokens),
        today_cost: None,
    })
}

fn pi_session_file(root: &Path, target: &UsageTarget) -> Option<PathBuf> {
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    let dirs = fs::read_dir(root).ok()?;
    for dir in dirs.flatten() {
        if !dir.metadata().ok()?.is_dir() {
            continue;
        }
        let Ok(files) = fs::read_dir(dir.path()) else {
            continue;
        };
        for file in files.flatten() {
            let path = file.path();
            if path.extension().is_none_or(|ext| ext != "jsonl") {
                continue;
            }
            let matches_target = target.session_id.as_deref().is_some_and(|id| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with(&format!("{id}.jsonl")))
            });
            if !matches_target && !path_matches_cwd(&path, &target.cwd) {
                continue;
            }
            let Ok(modified) = file.metadata().and_then(|m| m.modified()) else {
                continue;
            };
            if best.as_ref().is_none_or(|(at, _)| modified > *at) {
                best = Some((modified, path));
            }
        }
    }
    best.map(|(_, path)| path)
}

/// Pi names its session directory after the cwd with `/` replaced by `-`,
/// wrapped in double dashes: `/home/me/app` → `--home-me-app--`.
fn path_matches_cwd(path: &Path, cwd: &str) -> bool {
    if cwd.is_empty() {
        return false;
    }
    let slug = format!("--{}--", cwd.trim_matches('/').replace('/', "-"));
    path.components()
        .any(|part| part.as_os_str().to_string_lossy() == slug)
}

// ─── Shared helpers ─────────────────────────────────────────────────

fn config_home(suffix: &str) -> PathBuf {
    let base = std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
        .unwrap_or_default();
    base.join(suffix)
}

fn http_get_json(url: &str, headers: &[(&str, String)]) -> Result<Value, String> {
    http::get(url, headers)
}

fn stale_notice(value: &Value, extras: &mut Vec<String>) {
    if let Some(age) = value["_agentstmux_stale_seconds"].as_u64() {
        extras.push(format!("Cached {}m ago · retrying", age / 60));
    }
}

/// A harness can use a different account from the native CLI. Never show
/// native CLI limits as if they belonged to the harness's API-key account.
fn harness_credentials(target: &UsageTarget, claude: bool) -> Result<(String, String), String> {
    let provider = if claude { "anthropic" } else { "openai" };
    let credential = if target.is_pi {
        let raw = fs::read_to_string(config_home(".pi/agent/auth.json"))
            .map_err(|_| "Pi provider credentials unavailable".to_string())?;
        let auth: Value =
            serde_json::from_str(&raw).map_err(|_| "Pi credentials unreadable".to_string())?;
        auth[if claude { "anthropic" } else { "openai-codex" }].clone()
    } else {
        let db = config_home(".local/share/opencode/opencode.db");
        let sql = format!(
            "select value from credential where integration_id='{provider}' and active=1 order by time_updated desc limit 1;"
        );
        let row = sqlite3(&db, &sql).unwrap_or_default();
        serde_json::from_str::<Value>(&row)
            .ok()
            .or_else(|| {
                let raw =
                    fs::read_to_string(config_home(".local/share/opencode/auth.json")).ok()?;
                let auth: Value = serde_json::from_str(&raw).ok()?;
                Some(auth[provider].clone())
            })
            .unwrap_or(Value::Null)
    };
    parse_harness_credential(&credential)
}

fn parse_harness_credential(credential: &Value) -> Result<(String, String), String> {
    if credential["type"].as_str() != Some("oauth") {
        return Err("Provider has no subscription usage".into());
    }
    let token = credential["access"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "Provider sign-in unavailable".to_string())?;
    let account = credential
        .pointer("/metadata/accountID")
        .or_else(|| credential.get("accountId"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    Ok((token.to_string(), account.to_string()))
}

/// Parse an RFC 3339 timestamp (`2026-11-07T14:09:59.697428+00:00`) into
/// epoch seconds without pulling in a date-time crate.
pub fn parse_rfc3339(input: &str) -> Option<u64> {
    let bytes = input.as_bytes();
    if bytes.len() < 19 {
        return None;
    }
    let year: i64 = input.get(0..4)?.parse().ok()?;
    let month: i64 = input.get(5..7)?.parse().ok()?;
    let day: i64 = input.get(8..10)?.parse().ok()?;
    let hour: i64 = input.get(11..13)?.parse().ok()?;
    let minute: i64 = input.get(14..16)?.parse().ok()?;
    let second: i64 = input.get(17..19)?.parse().ok()?;

    let mut offset_seconds: i64 = 0;
    let rest = &input[19..];
    if let Some(idx) = rest.rfind(['+', '-']) {
        let sign = if rest.as_bytes()[idx] == b'-' { -1 } else { 1 };
        let offset = &rest[idx + 1..];
        if offset.len() >= 5 {
            let hours: i64 = offset.get(0..2)?.parse().ok()?;
            let minutes: i64 = offset.get(3..5)?.parse().ok()?;
            offset_seconds = sign * (hours * 3600 + minutes * 60);
        }
    }

    let days = days_from_civil(year, month, day);
    let secs = days * 86_400 + hour * 3600 + minute * 60 + second - offset_seconds;
    u64::try_from(secs).ok()
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_prime = (month + 9) % 12;
    let day_of_year = (153 * month_prime + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// `in 1h 12m`, `in 4d 3h`, `now`.
pub fn format_countdown(reset_at: u64, now: u64) -> String {
    let secs = reset_at.saturating_sub(now);
    if secs == 0 {
        return "now".into();
    }
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3600;
    let minutes = (secs % 3600) / 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes:02}m")
    } else {
        format!("{minutes}m")
    }
}

/// `$12.34 · 1.2M tok` style token count.
pub fn format_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M tok", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.0}k tok", tokens as f64 / 1_000.0)
    } else {
        format!("{tokens} tok")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn claude_parser_uses_real_windows_and_marks_stale_data() {
        let value = json!({
            "five_hour": {"utilization":95.0,"resets_at":"2026-11-07T14:09:59Z"},
            "seven_day": {"utilization":51.0,"resets_at":"2026-11-09T19:59:59Z"},
            "_agentstmux_stale_seconds":120
        });
        let usage = parse_claude_usage(&value, Some("team".into())).unwrap();
        assert_eq!(usage.windows.len(), 2);
        assert_eq!(usage.windows[0].percent, 95.0);
        assert_eq!(usage.windows[1].label, "1w");
        assert!(usage.extras[0].contains("Cached 2m"));
    }

    #[test]
    fn api_key_harness_does_not_borrow_native_cli_subscription() {
        assert!(parse_harness_credential(&json!({"type":"api","key":"test"})).is_err());
        let credential = parse_harness_credential(&json!({
            "type":"oauth","access":"harness-account-token",
            "metadata":{"accountID":"harness-account"}
        }))
        .unwrap();
        assert_eq!(credential.0, "harness-account-token");
        assert_eq!(credential.1, "harness-account");
    }

    #[test]
    fn claude_identity_requires_account_and_organization() {
        let profile = json!({"account":{"uuid":"account-a"},"organization":{"uuid":"org-a"}});
        let identity = claude_account_identity(&profile).unwrap();
        assert_eq!(identity, ("account-a".into(), "org-a".into()));
        assert_ne!(
            Some(identity),
            claude_account_identity(&json!({
                "account":{"uuid":"account-a"},"organization":{"uuid":"different-org"}
            }))
        );
        assert!(claude_account_identity(&json!({"account":{"uuid":"account-a"}})).is_none());
        assert!(
            claude_account_identity(
                &json!({"account":{"uuid":""},"organization":{"uuid":"org-a"}})
            )
            .is_none()
        );
    }

    // ─── timestamp parsing / formatting ─────────────────────────────

    #[test]
    fn parses_utc_timestamp_with_fraction() {
        assert_eq!(
            parse_rfc3339("2026-11-07T14:09:59.697428+00:00"),
            Some(1_794_060_599)
        );
    }

    #[test]
    fn parses_positive_offset() {
        // 14:09:59+02:00 == 12:09:59Z
        assert_eq!(
            parse_rfc3339("2026-11-07T14:09:59+02:00"),
            Some(1_794_053_399)
        );
    }

    #[test]
    fn parses_negative_offset() {
        // 14:09:59-05:00 == 19:09:59Z
        assert_eq!(
            parse_rfc3339("2026-11-07T14:09:59-05:00"),
            Some(1_794_078_599)
        );
    }

    #[test]
    fn rejects_garbage_timestamp() {
        assert_eq!(parse_rfc3339("not a date"), None);
    }

    #[test]
    fn countdown_formats_days_hours_minutes() {
        let now = 1_000_000;
        assert_eq!(format_countdown(now, now), "now");
        assert_eq!(format_countdown(now + 90, now), "1m");
        assert_eq!(format_countdown(now + 3_720, now), "1h 02m");
        assert_eq!(format_countdown(now + 4 * 86_400 + 3 * 3600, now), "4d 3h");
    }

    #[test]
    fn token_counts_are_compact() {
        assert_eq!(format_tokens(999), "999 tok");
        assert_eq!(format_tokens(1_500), "2k tok");
        assert_eq!(format_tokens(6_349_021), "6.3M tok");
    }

    #[test]
    fn window_labels_derive_from_seconds() {
        assert_eq!(window_label(18_000), "5h");
        assert_eq!(window_label(604_800), "1w");
        assert_eq!(window_label(300), "5m");
    }

    // ─── Claude parsing ────────────────────────────────────────────

    #[test]
    fn claude_usage_maps_five_hour_and_seven_day() {
        let value = json!({
            "five_hour": {"utilization": 91.0, "resets_at": "2026-11-07T14:09:59+00:00"},
            "seven_day": {"utilization": 51.0, "resets_at": "2026-11-09T19:59:59+00:00"},
            "extra_usage": {"is_enabled": true, "monthly_limit": 2000, "used_credits": 2224.0}
        });
        let mut windows = Vec::new();
        for (path, label) in [("/five_hour", "5h"), ("/seven_day", "1w")] {
            let window = value.pointer(path).unwrap();
            windows.push(UsageWindow {
                label: label.into(),
                percent: window["utilization"].as_f64().unwrap() as f32,
                resets_at: window["resets_at"].as_str().and_then(parse_rfc3339),
            });
        }
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].percent, 91.0);
        assert_eq!(windows[0].resets_at, Some(1_794_060_599));
        assert_eq!(windows[1].label, "1w");

        let used = value["extra_usage"]["used_credits"].as_f64().unwrap() / 100.0;
        let limit = value["extra_usage"]["monthly_limit"].as_f64().unwrap() / 100.0;
        assert_eq!(
            format!("Extra usage ${used:.2} of ${limit:.2}"),
            "Extra usage $22.24 of $20.00"
        );
    }

    // ─── Codex parsing ─────────────────────────────────────────────

    #[test]
    fn codex_usage_maps_primary_and_secondary_windows() {
        let value = json!({
            "plan_type": "team",
            "rate_limit": {
                "limit_reached": false,
                "primary_window": {
                    "used_percent": 37.0,
                    "limit_window_seconds": 18000,
                    "reset_at": 1_794_056_414u64
                },
                "secondary_window": {
                    "used_percent": 56.0,
                    "limit_window_seconds": 604800,
                    "reset_at": 1_794_346_765u64
                }
            },
            "credits": {"has_credits": true, "unlimited": false, "balance": 12.5}
        });
        let usage = parse_codex_usage(&value).expect("should parse");
        assert_eq!(usage.plan.as_deref(), Some("team"));
        assert_eq!(usage.windows.len(), 2);
        assert_eq!(usage.windows[0].label, "5h");
        assert_eq!(usage.windows[0].percent, 37.0);
        assert_eq!(usage.windows[1].label, "1w");
        assert_eq!(usage.extras, vec!["Credits 12.50"]);
    }

    #[test]
    fn codex_limit_reached_is_surfaced() {
        let value = json!({
            "rate_limit": {
                "limit_reached": true,
                "primary_window": {"used_percent": 100.0, "limit_window_seconds": 18000}
            }
        });
        let usage = parse_codex_usage(&value).expect("should parse");
        assert_eq!(usage.extras[0], "Limit reached");
        assert_eq!(usage.windows[0].percent, 100.0);
        assert_eq!(usage.windows[0].resets_at, None);
    }

    #[test]
    fn codex_missing_rate_limit_is_an_error() {
        assert!(parse_codex_usage(&json!({"plan_type": "team"})).is_err());
    }

    // ─── harness local usage ───────────────────────────────────────

    #[test]
    fn opencode_row_parses_cost_tokens_and_model() {
        let row = "15.217|1162786|{\"id\":\"deepseek/deepseek-v4.1-flash\",\"providerID\":\"commandcode\"}";
        let (provider, model, cost, tokens) = parse_opencode_row(row);
        assert_eq!(provider, "commandcode");
        assert_eq!(model, "deepseek/deepseek-v4.1-flash");
        assert_eq!(cost, Some(15.217));
        assert_eq!(tokens, Some(1_162_786));
    }

    #[test]
    fn sql_escaping_doubles_quotes() {
        assert_eq!(sql_escape("/home/o'brien/app"), "/home/o''brien/app");
    }

    #[test]
    fn provider_mapping_recognises_subscriptions() {
        assert_eq!(provider_for("anthropic"), Some(ProviderKind::Claude));
        assert_eq!(provider_for("openai-codex"), Some(ProviderKind::Codex));
        assert_eq!(provider_for("commandcode"), None);
        assert_eq!(provider_for("antigravity"), None);
    }

    #[test]
    fn pi_slug_matches_cwd() {
        assert!(path_matches_cwd(
            Path::new("/home/ahmad/.pi/agent/sessions/--home-ahmad--/a.jsonl"),
            "/home/ahmad"
        ));
        assert!(path_matches_cwd(
            Path::new("/home/ahmad/.pi/agent/sessions/--home-ahmad-app--/a.jsonl"),
            "/home/ahmad/app"
        ));
        assert!(!path_matches_cwd(
            Path::new("/home/ahmad/.pi/agent/sessions/--home-ahmad-app--/a.jsonl"),
            "/home/ahmad/other"
        ));
    }

    #[test]
    fn local_lines_reports_session_and_today() {
        let local = LocalUsage {
            provider: "commandcode".into(),
            model: "deepseek".into(),
            session_cost: Some(15.217),
            session_tokens: Some(1_162_786),
            today_cost: Some(20.5),
        };
        let lines = local_lines(&local);
        assert_eq!(lines[0], ("Using".into(), "commandcode".into()));
        assert_eq!(lines[1], ("Model".into(), "deepseek".into()));
        assert_eq!(lines[2], ("Session".into(), "$15.22 · 1.2M tok".into()));
        assert_eq!(lines[3], ("Today".into(), "$20.50".into()));
    }

    #[test]
    fn fetcher_unknown_agent_has_a_message() {
        let mut fetcher = UsageFetcher::new();
        let snapshot = fetcher.fetch(&UsageTarget {
            key: "k".into(),
            agent: AgentType::Unknown,
            is_pi: false,
            session_id: None,
            cwd: "/tmp".into(),
        });
        assert_eq!(snapshot.title, "Usage");
        assert!(snapshot.windows.is_empty());
        assert!(snapshot.message.is_some());
    }
}
