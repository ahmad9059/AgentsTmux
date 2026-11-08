//! Command Code billing API, verified against CLI v1.87.1. All caps come
//! from the authenticated account; no price table or guessed allowance.
use super::*;

const BASE: &str = "https://api.commandcode.ai";

pub(super) fn native_credentials(target: &UsageTarget) -> Result<String, String> {
    let mut config = serde_json::Map::new();
    for path in [
        config_home(".commandcode/config.json"),
        config_home(".commandcode/settings.json"),
        Path::new(&target.cwd).join(".commandcode/settings.json"),
        Path::new(&target.cwd).join(".commandcode/settings.local.json"),
    ] {
        if let Value::Object(values) = read_json(&path) {
            config.extend(values);
        }
    }
    if config.get("localOnly").and_then(Value::as_bool) == Some(true) {
        return Err("Command Code local-only mode".into());
    }
    let provider = config
        .get("modelProvider")
        .or_else(|| config.get("provider"))
        .and_then(Value::as_str)
        .unwrap_or("command");
    if !matches!(provider, "command" | "commandcode") {
        return Err(format!("No Command Code limits for {provider}"));
    }
    if let Ok(key) = std::env::var("COMMAND_CODE_API_KEY")
        && !key.trim().is_empty()
    {
        return Ok(key.trim().into());
    }
    let auth = read_json(&config_home(".commandcode/auth.json"));
    key(&auth).ok_or_else(|| "Sign in with `commandcode login`".into())
}

fn read_json(path: &Path) -> Value {
    fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or(Value::Null)
}

fn key(value: &Value) -> Option<String> {
    ["key", "apiKey"].into_iter().find_map(|field| {
        value[field]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string)
    })
}

fn credentials(target: &UsageTarget, provider: &str) -> Result<String, String> {
    // Never silently use a native CLI account for a harness with another key.
    let credential = if target.is_pi {
        read_json(&config_home(".pi/agent/auth.json"))[provider].clone()
    } else {
        let db = config_home(".local/share/opencode/opencode.db");
        let sql = format!(
            "select value from credential where integration_id='{}' and active=1 order by time_updated desc limit 1;",
            sql_escape(provider)
        );
        sqlite3(&db, &sql)
            .and_then(|row| serde_json::from_str(&row).ok())
            .unwrap_or_else(|| {
                read_json(&config_home(".local/share/opencode/auth.json"))[provider].clone()
            })
    };
    key(&credential).ok_or_else(|| "Command Code provider key unavailable".into())
}

pub(super) fn attach(snapshot: &mut UsageSnapshot, target: &UsageTarget, provider: &str) {
    match credentials(target, provider).and_then(|token| fetch(&token)) {
        Ok(usage) => apply_provider(snapshot, usage),
        Err(error) => snapshot.message = Some(error),
    }
}

pub(super) fn fetch(token: &str) -> Result<ProviderUsage, String> {
    let headers = [
        ("Authorization", format!("Bearer {token}")),
        ("User-Agent", "command-code/1.87.1 agents-tmux".into()),
    ];
    let who = http_get_json(&format!("{BASE}/alpha/whoami?limits=1"), &headers)?;
    let org = who.pointer("/org/id").and_then(Value::as_str);
    let url = |endpoint: &str| match org {
        Some(org) => format!("{BASE}{endpoint}?orgId={}", percent_encode(org)),
        None => format!("{BASE}{endpoint}"),
    };
    let credits = http_get_json(&url("/alpha/billing/credits"), &headers)?;
    let subscription = http_get_json(&url("/alpha/billing/subscriptions"), &headers)?;
    let mut summary_url = url("/alpha/usage/summary");
    if let Some(since) = subscription
        .pointer("/data/currentPeriodStart")
        .and_then(Value::as_str)
    {
        summary_url.push(if org.is_some() { '&' } else { '?' });
        summary_url.push_str("since=");
        summary_url.push_str(&percent_encode(since));
    }
    let summary = http_get_json(&summary_url, &headers)?;
    parse(&who, &credits, &subscription, &summary)
}

fn percent_encode(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

fn timestamp(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .map(|n| if n > 100_000_000_000 { n / 1000 } else { n })
        .or_else(|| value.as_str().and_then(parse_rfc3339))
}

fn parse(
    who: &Value,
    credits: &Value,
    subscription: &Value,
    summary: &Value,
) -> Result<ProviderUsage, String> {
    if !credits["credits"].is_object() {
        return Err("Command Code usage unavailable".into());
    }
    let mut windows = Vec::new();
    let mut extras = Vec::new();
    for value in [who, credits, subscription, summary] {
        stale_notice(value, &mut extras);
    }
    extras.dedup();
    for (name, label) in [("fiveHour", "5h"), ("weekly", "1w"), ("monthly", "1mo")] {
        let window = &credits["windowLimits"][name];
        if let (Some(used), Some(cap)) = (window["used"].as_f64(), window["cap"].as_f64())
            && used.is_finite()
            && cap.is_finite()
            && cap > 0.0
        {
            windows.push(UsageWindow {
                label: label.into(),
                percent: (used.max(0.0) / cap * 100.0) as f32,
                resets_at: timestamp(&window["resetAt"]),
            });
            extras.push(format!("{label} ${used:.2} / ${cap:.2}"));
        }
    }
    // Monthly credits spent plus remaining is the provider-reported pool,
    // not a hardcoded plan price. Paid/free extras are kept separate.
    if !windows.iter().any(|w| w.label == "1mo")
        && subscription.pointer("/data/status").and_then(Value::as_str) == Some("active")
        && let (Some(used), Some(left)) = (
            summary["totalMonthlyCredits"].as_f64(),
            credits["credits"]["monthlyCredits"].as_f64(),
        )
        && used >= 0.0
        && left >= 0.0
        && used + left > 0.0
    {
        let cap = used + left;
        windows.push(UsageWindow {
            label: "1mo".into(),
            percent: (used / cap * 100.0) as f32,
            resets_at: subscription
                .pointer("/data/currentPeriodEnd")
                .and_then(timestamp),
        });
        extras.push(format!("1mo ${used:.2} / ${cap:.2}"));
    }
    if let Some(total) = summary["totalCost"].as_f64() {
        extras.push(format!("Total ${total:.2} · billing period"));
    }
    if let (Some(input), Some(output)) = (
        summary["totalTokensIn"].as_u64(),
        summary["totalTokensOut"].as_u64(),
    ) {
        extras.push(format!(
            "In {} · Out {}",
            format_tokens(input),
            format_tokens(output)
        ));
    }
    if let Some(count) = summary["totalCount"].as_u64() {
        extras.push(format!("{count} requests · billing period"));
    }
    for (name, label) in [("purchasedCredits", "Extra"), ("freeCredits", "Free")] {
        if let Some(balance) = credits["credits"][name].as_f64()
            && balance > 0.0
        {
            extras.push(format!("{label} ${balance:.2} remaining"));
        }
    }
    if windows.is_empty() {
        extras.push("Pay-as-you-go · no usage windows".into());
    }
    Ok(ProviderUsage {
        plan: subscription
            .pointer("/data/planId")
            .and_then(Value::as_str)
            .map(str::to_string),
        windows,
        extras,
    })
}

pub(super) fn local_details(target: &UsageTarget) -> Vec<(String, String)> {
    let mut rows = vec![("Using".into(), "commandcode".into())];
    let Some(id) = &target.session_id else {
        return rows;
    };
    let data = read_json(
        &config_home(".local/state/agentstmux/commandcode")
            .join(format!("{}.json", percent_encode(id))),
    );
    if data["session_id"].as_str() != Some(id) {
        return rows;
    }
    if let Some(model) = data["model"].as_str().filter(|s| !s.is_empty()) {
        rows.push(("Model".into(), model.into()));
    }
    if let (Some(input), Some(output)) = (data["input"].as_u64(), data["output"].as_u64()) {
        rows.push((
            "Session".into(),
            format_tokens(input.saturating_add(output)),
        ));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    #[ignore = "requires the user's OpenCode Command Code credentials and network"]
    fn live_commandcode_account_reports_windows() {
        let target = UsageTarget {
            key: "live".into(),
            agent: AgentType::OpenCode,
            is_pi: false,
            session_id: None,
            cwd: String::new(),
        };
        let token = credentials(&target, "commandcode").unwrap();
        let usage = fetch(&token).unwrap();
        assert!(usage.windows.iter().any(|w| w.label == "5h"));
        assert!(usage.windows.iter().any(|w| w.label == "1w"));
        assert!(usage.windows.iter().any(|w| w.label == "1mo"));
        for window in &usage.windows {
            assert!(window.percent.is_finite());
            assert!(window.resets_at.is_some());
        }
        println!(
            "Command Code: {} verified windows; plan {}",
            usage.windows.len(),
            usage.plan.as_deref().unwrap_or("unknown")
        );
    }

    #[test]
    fn account_windows_and_monthly_pool_use_reported_values() {
        let credits = json!({"credits":{"monthlyCredits":4.0}, "windowLimits":{
            "fiveHour":{"used":1.5,"cap":3.0,"resetAt":1794174081000u64},
            "weekly":{"used":2.0,"cap":6.0,"resetAt":1794651316000u64}
        }});
        let subscription = json!({"data":{"status":"active","planId":"individual-go-v1","currentPeriodEnd":"2026-11-22T11:03:49Z"}});
        let summary = json!({"totalMonthlyCredits":2.0,"totalCost":2.5,"totalTokensIn":1000,"totalTokensOut":100,"totalCount":15});
        let usage = parse(&json!({}), &credits, &subscription, &summary).unwrap();
        assert_eq!(usage.windows.len(), 3);
        assert_eq!(usage.windows[0].percent, 50.0);
        assert_eq!(usage.windows[0].resets_at, Some(1794174081));
        assert_eq!(usage.windows[2].label, "1mo");
        assert!((usage.windows[2].percent - 33.3333).abs() < 0.001);
        assert!(
            usage
                .extras
                .iter()
                .any(|s| s == "Total $2.50 · billing period")
        );
    }

    #[test]
    fn payg_has_no_invented_limits_and_missing_response_is_an_error() {
        let usage = parse(
            &json!({}),
            &json!({"credits":{"purchasedCredits":8}}),
            &json!({"data":null}),
            &json!({}),
        )
        .unwrap();
        assert!(usage.windows.is_empty());
        assert!(usage.extras.iter().any(|s| s.contains("Pay-as-you-go")));
        assert!(parse(&json!({}), &json!({}), &json!({}), &json!({})).is_err());
    }

    #[test]
    fn unsafe_url_bytes_are_encoded_and_api_keys_are_not_oauth() {
        assert_eq!(percent_encode("org/a?b&x"), "org%2Fa%3Fb%26x");
        assert_eq!(
            key(&json!({"type":"api","key":"test-key"})).as_deref(),
            Some("test-key")
        );
        assert!(key(&json!({"type":"oauth","access":"not-a-command-key"})).is_none());
    }
}
