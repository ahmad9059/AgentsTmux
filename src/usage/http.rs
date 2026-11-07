//! Shared, credential-scoped usage cache. Never persist credentials or put
//! bearer tokens in subprocess arguments. Provider throttling applies across
//! sidebar processes, not just within one worker.
use std::collections::hash_map::DefaultHasher;
use std::fs::{self, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

const FRESH_SECONDS: u64 = 60;
const MAX_STALE_SECONDS: u64 = 3600;

pub(super) fn get(url: &str, headers: &[(&str, String)]) -> Result<Value, String> {
    let root = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| super::config_home(".cache"))
        .join("agents-tmux/usage");
    fs::create_dir_all(&root).map_err(|_| "Cannot create usage cache".to_string())?;
    let mut hasher = DefaultHasher::new();
    url.hash(&mut hasher);
    headers.hash(&mut hasher);
    let path = root.join(format!("{:016x}.json", hasher.finish()));
    cached_get(&path, super::now_epoch_secs(), || request(url, headers))
}

fn cached_get(
    path: &Path,
    now: u64,
    fetch: impl FnOnce() -> Result<Value, Failure>,
) -> Result<Value, String> {
    let cached = fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .unwrap_or_else(|| json!({}));
    if cached["retry_at"].as_u64().unwrap_or(0) > now {
        return cached_result(&cached, now);
    }
    let lock_path = path.with_extension("lock");
    // A killed worker must not strand the lock forever. Each request has an
    // eight-second timeout, so a ninety-second lock cannot still be live.
    if fs::metadata(&lock_path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|at| at.elapsed().ok())
        .is_some_and(|age| age.as_secs() > 90)
    {
        let _ = fs::remove_file(&lock_path);
    }
    let Ok(_file) = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
    else {
        return cached_result(&cached, now);
    };
    let _lock = Lock(lock_path);
    // Another process may have just completed its request before we locked.
    let cached = fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .unwrap_or(cached);
    if cached["retry_at"].as_u64().unwrap_or(0) > now {
        return cached_result(&cached, now);
    }
    let updated = match fetch() {
        Ok(value) => json!({"data": value, "fetched_at": now, "retry_at": now + FRESH_SECONDS}),
        Err(failure) => {
            let failures = cached["failures"].as_u64().unwrap_or(0).saturating_add(1);
            let delay = failure
                .retry_after
                .unwrap_or_else(|| (60u64 * (1u64 << failures.min(4))).min(900))
                .max(60);
            json!({"data": cached["data"], "fetched_at": cached["fetched_at"],
                "retry_at": now.saturating_add(delay), "error": failure.message,
                "failures": failures})
        }
    };
    // Atomic replace: readers never see a partly written response.
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    if let Ok(bytes) = serde_json::to_vec(&updated)
        && fs::write(&temp, bytes).is_ok()
    {
        let _ = fs::rename(&temp, path);
    }
    cached_result(&updated, now)
}

fn cached_result(cache: &Value, now: u64) -> Result<Value, String> {
    let age = now.saturating_sub(cache["fetched_at"].as_u64().unwrap_or(0));
    if cache["data"].is_object() && age <= MAX_STALE_SECONDS {
        let mut value = cache["data"].clone();
        if cache["error"].is_string() {
            value["_agentstmux_stale_seconds"] = json!(age);
        }
        return Ok(value);
    }
    let message = cache["error"].as_str().unwrap_or("Fetching provider usage");
    let seconds = cache["retry_at"]
        .as_u64()
        .unwrap_or(now)
        .saturating_sub(now);
    Err(format!("{message}; retry in {seconds}s"))
}

struct Lock(PathBuf);
impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

struct Failure {
    message: String,
    retry_after: Option<u64>,
}

fn request(url: &str, headers: &[(&str, String)]) -> Result<Value, Failure> {
    let fail = |message: &str| Failure {
        message: message.into(),
        retry_after: None,
    };
    let mut child = Command::new("curl")
        .args([
            "-sS",
            "--connect-timeout",
            "3",
            "--max-time",
            "8",
            "--dump-header",
            "-",
            "--config",
            "-",
            url,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| fail("curl is unavailable"))?;
    let config = headers
        .iter()
        .map(|(name, value)| {
            format!(
                "header = \"{}: {}\"\n",
                name,
                value
                    .replace('\\', "\\\\")
                    .replace('"', "\\\"")
                    .replace(['\r', '\n'], "")
            )
        })
        .collect::<String>();
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(config.as_bytes());
    }
    let output = child
        .wait_with_output()
        .map_err(|_| fail("Usage request failed"))?;
    if !output.status.success() {
        return Err(fail("Usage connection failed"));
    }
    let value = parse_response(&String::from_utf8_lossy(&output.stdout))?;
    if url.ends_with("/api/oauth/profile") {
        // Identity verification needs UUIDs only, not email/name or the
        // other personal fields returned by the profile endpoint.
        return Ok(json!({"account":{"uuid":value.pointer("/account/uuid")},
            "organization":{"uuid":value.pointer("/organization/uuid")}}));
    }
    Ok(value)
}

fn parse_response(raw: &str) -> Result<Value, Failure> {
    let (headers, body) = raw.rsplit_once("\r\n\r\n").unwrap_or(("", raw));
    let status = headers
        .lines()
        .rfind(|line| line.starts_with("HTTP/"))
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|status| status.parse::<u16>().ok())
        .unwrap_or(0);
    let retry_after = headers
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(key, _)| key.eq_ignore_ascii_case("retry-after"))
        .and_then(|(_, value)| value.trim().parse::<f64>().ok())
        .filter(|n| n.is_finite() && *n >= 0.0)
        .map(|n| n.ceil() as u64);
    let value = serde_json::from_str::<Value>(body).ok();
    if (200..300).contains(&status)
        && let Some(value) = value.as_ref()
        && value.get("error").is_none()
    {
        return Ok(value.clone());
    }
    let message = match status {
        429 => "Provider rate limited",
        401 | 403 => "Provider sign-in expired or denied",
        _ => "Provider usage request failed",
    };
    Err(Failure {
        message: message.into(),
        retry_after,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_json_after_proxy_and_origin_headers() {
        let raw = "HTTP/1.1 200 Connection established\r\n\r\nHTTP/2 200\r\ncontent-type: application/json\r\n\r\n{\"five_hour\":{\"utilization\":95}}";
        let value = parse_response(raw).ok().unwrap();
        assert_eq!(value["five_hour"]["utilization"], 95);
    }
    #[test]
    fn detects_error_without_top_level_type() {
        let err = parse_response(
            "HTTP/2 429\r\nretry-after: 180\r\n\r\n{\"error\":{\"type\":\"rate_limit_error\"}}",
        )
        .unwrap_err();
        assert_eq!(err.message, "Provider rate limited");
        assert_eq!(err.retry_after, Some(180));
    }
    #[test]
    fn retains_good_response_and_obeys_shared_backoff() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.json");
        let data = json!({"five_hour":{"utilization":51}});
        assert_eq!(cached_get(&path, 1000, || Ok(data.clone())).unwrap(), data);
        assert_eq!(
            cached_get(&path, 1010, || panic!("fresh cache must skip network")).unwrap(),
            data
        );
        let stale = cached_get(&path, 1061, || {
            Err(Failure {
                message: "Provider rate limited".into(),
                retry_after: Some(300),
            })
        })
        .unwrap();
        assert_eq!(stale["five_hour"], data["five_hour"]);
        assert_eq!(stale["_agentstmux_stale_seconds"], 61);
        assert!(cached_get(&path, 1100, || panic!("must obey retry-after")).is_ok());
        assert_eq!(cached_get(&path, 1362, || Ok(data.clone())).unwrap(), data);
    }
    #[test]
    fn first_failure_is_not_cached_as_usage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.json");
        let err = cached_get(&path, 1000, || {
            Err(Failure {
                message: "Provider rate limited".into(),
                retry_after: Some(120),
            })
        })
        .unwrap_err();
        assert!(err.contains("rate limited"));
        assert!(cached_get(&path, 1010, || panic!("backoff")).is_err());
    }
    #[test]
    fn stale_data_expires_instead_of_looking_live_forever() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.json");
        cached_get(&path, 1000, || Ok(json!({"five_hour":{}}))).unwrap();
        assert!(
            cached_get(&path, 5000, || Err(Failure {
                message: "Offline".into(),
                retry_after: None
            }))
            .is_err()
        );
    }
}
