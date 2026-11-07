//! Native CLI telemetry. Costs come from reported cost snapshots, never a
//! guessed model price. Token totals are not subscription quota percentages.
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::{UsageTarget, config_home, format_tokens, parse_rfc3339};
use crate::tmux::AgentType;
use serde_json::Value;

#[derive(Default)]
struct Stats {
    model: String,
    tokens: u64,
    today_tokens: u64,
    cost: Option<f64>,
    all_today: bool,
}

pub(super) fn details(target: &UsageTarget, now: u64) -> Vec<(String, String)> {
    let claude = target.agent == AgentType::Claude;
    let root = if claude {
        config_home(".claude/projects")
    } else {
        config_home(".codex/sessions")
    };
    let mut files = Vec::new();
    collect(&root, if claude { 1 } else { 3 }, &mut files);
    let midnight = local_midnight(now);
    let mut current = None;
    let mut today_tokens = 0u64;
    let mut today_cost = 0.0;
    let mut complete_cost = true;
    let mut complete_today = true;
    let mut seen = HashSet::new();
    for path in files {
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        if !seen.insert(name.to_string()) {
            continue;
        }
        let selected = target
            .session_id
            .as_ref()
            .is_some_and(|id| name.ends_with(&format!("{id}.jsonl")));
        let Ok(meta) = fs::metadata(&path) else {
            continue;
        };
        let modified_today = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .is_some_and(|d| d.as_secs() >= midnight);
        if !selected && !modified_today {
            continue;
        }
        // Bound work off-thread. If a journal exceeds this bound, omit the
        // daily aggregate rather than present a partial total as complete.
        if meta.len() > 32 * 1024 * 1024 {
            if modified_today {
                complete_today = false;
            }
            continue;
        }
        let Ok(raw) = fs::read_to_string(&path) else {
            if modified_today {
                complete_today = false;
            }
            continue;
        };
        let stats = if claude {
            claude_stats(&raw, midnight)
        } else {
            codex_stats(&raw, midnight)
        };
        if modified_today {
            today_tokens = today_tokens.saturating_add(stats.today_tokens);
            if stats.today_tokens > 0 {
                if stats.all_today
                    && let Some(cost) = stats.cost
                {
                    today_cost += cost;
                } else {
                    complete_cost = false;
                }
            }
        }
        if selected {
            current = Some(stats);
        }
    }
    let mut rows = vec![(
        "Using".into(),
        if claude { "anthropic" } else { "openai" }.into(),
    )];
    // New sessions may not have a journal until their first request. The
    // live CLI chrome already displays the selected model at that point.
    // Claude's welcome header may remain in view after a model switch;
    // once a journal exists its latest response is more authoritative.
    // Codex's footer, in contrast, reflects the current selection directly.
    let live_model = live_model(target, claude)
        .filter(|_| !claude || current.as_ref().is_none_or(|stats| stats.model.is_empty()));
    if let Some(model) = live_model.as_ref() {
        rows.push(("Model".into(), model.clone()));
    }
    if let Some(stats) = current {
        if live_model.is_none() && !stats.model.is_empty() {
            rows.push(("Model".into(), stats.model));
        }
        if stats.tokens > 0 || stats.cost.is_some() {
            let value = if let Some(cost) = stats.cost {
                format!("${cost:.2} · {}", format_tokens(stats.tokens))
            } else {
                format_tokens(stats.tokens)
            };
            rows.push(("Session".into(), value));
        }
    }
    if complete_today && today_tokens > 0 {
        rows.push((
            "Today".into(),
            if complete_cost {
                format!("${today_cost:.2}")
            } else {
                format_tokens(today_tokens)
            },
        ));
    }
    rows
}

fn live_model(target: &UsageTarget, claude: bool) -> Option<String> {
    let pane = target.key.split('|').next()?;
    // Only the actual pane identity may be passed to tmux, never a session
    // label or arbitrary target expression.
    if !pane.starts_with('%') || pane.len() < 2 || !pane[1..].bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let output = Command::new("tmux")
        .args(["capture-pane", "-p", "-t", pane])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let screen = String::from_utf8_lossy(&output.stdout);
    model_from_screen(&screen, claude)
}

/// Read CLI chrome only. Do not search conversation text for model names:
/// a user discussing a different model must not change this row.
fn model_from_screen(screen: &str, claude: bool) -> Option<String> {
    if claude {
        let lines: Vec<_> = screen.lines().take(8).collect();
        let header = lines
            .iter()
            .position(|line| line.contains("Claude Code v"))?;
        let line = lines.get(header + 1)?;
        let (left, plan) = line.split_once('·')?;
        if !plan.trim().starts_with("Claude ") {
            return None;
        }
        let words: Vec<_> = left.split_whitespace().collect();
        let index = words
            .iter()
            .position(|word| matches!(*word, "Opus" | "Sonnet" | "Haiku"))?;
        let version = *words.get(index + 1)?;
        if !version.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
            return None;
        }
        Some(format!("{} {version}", words[index]))
    } else {
        // Codex's persistent footer follows the input prompt and includes
        // `<model> <effort> · <cwd>`. Restrict the search to the bottom chrome.
        let lines: Vec<_> = screen.lines().collect();
        for line in lines.iter().rev().take(4) {
            let Some((left, right)) = line.split_once('·') else {
                continue;
            };
            if !right.trim().starts_with(['~', '/']) {
                continue;
            }
            let mut words = left.split_whitespace();
            let model = words.next()?;
            let effort = words.next().unwrap_or_default();
            if matches!(
                effort,
                "default" | "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max"
            ) && model
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-._/".contains(&b))
                && model.bytes().any(|b| b.is_ascii_digit())
            {
                return Some(model.to_string());
            }
        }
        None
    }
}

fn collect(root: &Path, depth: usize, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() && depth > 0 {
            collect(&path, depth - 1, files);
        } else if path.extension().is_some_and(|ext| ext == "jsonl") {
            files.push(path);
        }
    }
}

fn claude_stats(raw: &str, midnight: u64) -> Stats {
    // Streaming updates can repeat the same message ID. Keep the final
    // usage record for each message instead of counting it multiple times.
    let mut messages = HashMap::<String, (u64, bool)>::new();
    let mut stats = Stats::default();
    let mut cost_tokens = None;
    for (index, line) in raw.lines().enumerate() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if value["type"] == "assistant" {
            let message = &value["message"];
            if let Some(model) = message["model"].as_str().filter(|s| !s.starts_with('<')) {
                stats.model = model.into();
            }
            let usage = &message["usage"];
            if !usage.is_object() {
                continue;
            }
            let tokens = [
                "input_tokens",
                "output_tokens",
                "cache_read_input_tokens",
                "cache_creation_input_tokens",
            ]
            .iter()
            .map(|k| usage[k].as_u64().unwrap_or(0))
            .sum::<u64>();
            let today = value["timestamp"]
                .as_str()
                .and_then(parse_rfc3339)
                .is_some_and(|t| t >= midnight);
            let id = message["id"]
                .as_str()
                .or_else(|| value["uuid"].as_str())
                .map(str::to_string)
                .unwrap_or_else(|| index.to_string());
            messages.insert(id, (tokens, today));
        } else if value["type"] == "cost-state" {
            stats.cost = value["totalCostUSD"].as_f64();
            if let Some(models) = value["modelUsage"].as_object() {
                cost_tokens = Some(
                    models
                        .values()
                        .map(|m| {
                            [
                                "inputTokens",
                                "outputTokens",
                                "cacheReadInputTokens",
                                "cacheCreationInputTokens",
                            ]
                            .iter()
                            .map(|k| m[k].as_u64().unwrap_or(0))
                            .sum::<u64>()
                        })
                        .sum::<u64>(),
                );
                if stats.model.is_empty() {
                    stats.model = models.keys().next().cloned().unwrap_or_default();
                }
            }
        }
    }
    stats.tokens = messages.values().map(|(tokens, _)| *tokens).sum();
    stats.today_tokens = messages
        .values()
        .filter(|(_, today)| *today)
        .map(|(tokens, _)| *tokens)
        .sum();
    stats.all_today = stats.tokens == stats.today_tokens;
    // A saved cost snapshot can lag behind live messages; do not display
    // that old dollar amount as the cost of the newer token total.
    if cost_tokens != Some(stats.tokens) {
        stats.cost = None;
    }
    stats
}

fn codex_stats(raw: &str, midnight: u64) -> Stats {
    let mut stats = Stats::default();
    let mut prior = 0;
    for line in raw.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let payload = &value["payload"];
        if let Some(model) = payload["model"].as_str().or_else(|| {
            payload
                .pointer("/thread_settings/model")
                .and_then(Value::as_str)
        }) {
            stats.model = model.into();
        }
        if payload["type"] != "token_count" {
            continue;
        }
        let Some(total) = payload
            .pointer("/info/total_token_usage/total_tokens")
            .and_then(Value::as_u64)
        else {
            continue;
        };
        let delta = if total >= prior { total - prior } else { total };
        if value["timestamp"]
            .as_str()
            .and_then(parse_rfc3339)
            .is_some_and(|t| t >= midnight)
        {
            stats.today_tokens += delta;
        }
        stats.tokens = total;
        prior = total;
    }
    stats
}

fn local_midnight(now: u64) -> u64 {
    // SAFETY: both libc calls receive initialized, valid pointers.
    unsafe {
        let seconds = now as libc::time_t;
        let mut local: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&seconds, &mut local).is_null() {
            return now;
        }
        local.tm_hour = 0;
        local.tm_min = 0;
        local.tm_sec = 0;
        local.tm_isdst = -1;
        u64::try_from(libc::mktime(&mut local)).unwrap_or(now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn claude_model_is_available_before_a_journal_exists() {
        let screen =
            "\n ▐▛███▛█   Claude Code v2.1.302\n▝▜██████▀  Opus 5.5 · Claude Team\n ▝▝ ~/project\n";
        assert_eq!(model_from_screen(screen, true).as_deref(), Some("Opus 5.5"));
    }
    #[test]
    fn codex_model_comes_from_the_live_footer() {
        let screen = "OpenAI Codex\n\n› Ask Codex to do anything\n\n  GPT-6.1-Sol default · ~/project\n  ← for agents · ? for shortcuts\n";
        assert_eq!(
            model_from_screen(screen, false).as_deref(),
            Some("GPT-6.1-Sol")
        );
        let switched = screen.replace("GPT-6.1-Sol default", "gpt-6.2 high");
        assert_eq!(
            model_from_screen(&switched, false).as_deref(),
            Some("gpt-6.2")
        );
    }
    #[test]
    fn discussion_of_a_model_is_not_model_telemetry() {
        assert!(model_from_screen("Please compare Opus 5.5 · Claude Team", true).is_none());
        assert!(model_from_screen("GPT-6.1-Sol is great · ~/project", false).is_none());
    }
    #[test]
    fn claude_deduplicates_streaming_and_uses_reported_cost() {
        let raw = concat!(
            "{\"type\":\"assistant\",\"timestamp\":\"2026-11-07T12:00:00Z\",\"message\":{\"id\":\"a\",\"model\":\"claude-test\",\"usage\":{\"input_tokens\":10,\"output_tokens\":2}}}\n",
            "{\"type\":\"assistant\",\"timestamp\":\"2026-11-07T12:00:00Z\",\"message\":{\"id\":\"a\",\"usage\":{\"input_tokens\":10,\"output_tokens\":5}}}\n",
            "{\"type\":\"cost-state\",\"totalCostUSD\":0.42,\"modelUsage\":{\"claude-test\":{\"inputTokens\":10,\"outputTokens\":5}}}\n"
        );
        let stats = claude_stats(raw, parse_rfc3339("2026-11-07T00:00:00Z").unwrap());
        assert_eq!(stats.model, "claude-test");
        assert_eq!(stats.tokens, 15);
        assert_eq!(stats.today_tokens, 15);
        assert_eq!(stats.cost, Some(0.42));
    }
    #[test]
    fn codex_cumulative_snapshots_are_not_added_together() {
        let raw = concat!(
            "{\"timestamp\":\"2026-11-06T22:00:00Z\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"total_tokens\":100}}}}\n",
            "{\"timestamp\":\"2026-11-07T12:00:00Z\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"total_tokens\":130}}}}\n",
            "{\"timestamp\":\"2026-11-07T12:01:00Z\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"total_tokens\":140}}}}\n"
        );
        let stats = codex_stats(raw, parse_rfc3339("2026-11-07T00:00:00Z").unwrap());
        assert_eq!(stats.tokens, 140);
        assert_eq!(stats.today_tokens, 40);
        assert!(stats.cost.is_none());
    }
}
