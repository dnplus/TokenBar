//! Cursor plan window for the quota dump only.
//!
//! Syrtis does not fetch this. The menu-bar poll stays on `agent_usage::run`.
//! Auth is the desktop `state.vscdb` bearer, then the macOS keychain item
//! `cursor-access-token`. A refreshed access token stays in memory.
//! // poteto: do not write the refreshed token back into Cursor's database.

use crate::agent_usage::provider_http_client_builder;
use base64::Engine;
use chrono::{DateTime, SecondsFormat, TimeZone, Utc};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

const USAGE_URL: &str = "https://api2.cursor.sh/aiserver.v1.DashboardService/GetCurrentPeriodUsage";
const PLAN_URL: &str = "https://api2.cursor.sh/aiserver.v1.DashboardService/GetPlanInfo";
const REFRESH_URL: &str = "https://api2.cursor.sh/oauth/token";
// poteto: public Cursor Auth0 client id, hardcoded. Not a user secret.
const CLIENT_ID: &str = "KbZUR41cY7W6zRSdpSUJ7I7mLYBKOCmB";
pub(crate) const WINDOW_KEY: &str = "plan.period.v1";
const ACCESS_SERVICE: &str = "cursor-access-token";
const REFRESH_SERVICE: &str = "cursor-refresh-token";

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ParsedPlanWindow {
    pub(crate) used_percent: f64,
    pub(crate) remaining_percent: f64,
    pub(crate) reset_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LocalCursorAuth {
    pub(crate) access_token: String,
    pub(crate) refresh_token: Option<String>,
    pub(crate) email: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DbFileState {
    MissingHome,
    MissingFile,
    Read,
    Ciphertext,
}

pub(crate) fn account_label(raw: Option<&str>) -> Option<String> {
    let email = raw?.trim();
    if (3..=254).contains(&email.len())
        && email.matches('@').count() == 1
        && !email.contains(char::is_whitespace)
        && !email.contains("Bearer")
    {
        Some(email.to_string())
    } else {
        None
    }
}

pub(crate) fn access_token_usable(token: &str) -> bool {
    let parts: Vec<&str> = token.split('.').collect();
    parts.len() == 3 && parts.iter().all(|part| !part.is_empty())
}

pub(crate) fn needs_refresh(token: &str, now_unix: i64) -> bool {
    jwt_exp(token).is_some_and(|exp| exp <= now_unix.saturating_add(60))
}

fn jwt_exp(token: &str) -> Option<i64> {
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    value.get("exp").and_then(Value::as_i64)
}

/// Pick a bearer from the desktop database, then the keychain. Neither input
/// is logged. A non-JWT database value is treated as safeStorage ciphertext.
pub(crate) fn resolve_local_auth(
    db_state: DbFileState,
    db_access: Option<&str>,
    db_refresh: Option<&str>,
    db_email: Option<&str>,
    keychain_access: Option<&str>,
    keychain_refresh: Option<&str>,
) -> Result<LocalCursorAuth, String> {
    if let Some(access) = db_access
        .map(str::trim)
        .filter(|token| access_token_usable(token))
    {
        return Ok(LocalCursorAuth {
            access_token: access.to_string(),
            refresh_token: nonempty(db_refresh),
            email: account_label(db_email),
        });
    }
    if let Some(access) = keychain_access
        .map(str::trim)
        .filter(|token| access_token_usable(token))
    {
        return Ok(LocalCursorAuth {
            access_token: access.to_string(),
            refresh_token: nonempty(keychain_refresh).or_else(|| nonempty(db_refresh)),
            email: account_label(db_email),
        });
    }
    Err(auth_blocker(db_state))
}

fn nonempty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn auth_blocker(db_state: DbFileState) -> String {
    match db_state {
        DbFileState::MissingHome => {
            "HOME is unset, so Cursor state.vscdb and the macOS keychain could not be resolved."
                .to_string()
        }
        DbFileState::MissingFile => {
            "Cursor state.vscdb is missing (~/Library/Application Support/Cursor/User/globalStorage/state.vscdb), and keychain item cursor-access-token was not readable.".to_string()
        }
        DbFileState::Ciphertext => {
            "cursorAuth/accessToken is not a JWT. Cursor may have stored a safeStorage ciphertext, and keychain item cursor-access-token was not readable.".to_string()
        }
        DbFileState::Read => {
            "cursorAuth/accessToken was empty, and keychain item cursor-access-token was not readable.".to_string()
        }
    }
}

pub(crate) fn parse_plan_window(
    usage: &Value,
    plan: Option<&Value>,
) -> Result<ParsedPlanWindow, String> {
    let plan_usage = usage
        .get("planUsage")
        .filter(|value| value.is_object())
        .ok_or_else(|| plan_usage_missing(usage))?;
    let (used_percent, remaining_percent) = plan_percents(plan_usage)?;
    let reset_at = billing_reset_iso(usage.get("billingCycleEnd")).or_else(|| {
        billing_reset_iso(plan.and_then(|value| value.pointer("/planInfo/billingCycleEnd")))
    });
    Ok(ParsedPlanWindow {
        used_percent,
        remaining_percent,
        reset_at,
    })
}

fn plan_usage_missing(usage: &Value) -> String {
    match usage.get("code").and_then(Value::as_str) {
        Some(code) if is_short_code(code) => {
            format!("Cursor GetCurrentPeriodUsage returned code {code}.")
        }
        _ => "Cursor GetCurrentPeriodUsage had no planUsage object.".to_string(),
    }
}

fn is_short_code(code: &str) -> bool {
    (1..40).contains(&code.len())
        && code
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn plan_percents(plan_usage: &Value) -> Result<(f64, f64), String> {
    if let Some(used) = plan_usage.get("totalPercentUsed").and_then(Value::as_f64) {
        if used.is_finite() && (0.0..=100.0).contains(&used) {
            return Ok(round_pair(used));
        }
    }
    let remaining = plan_usage.get("remaining").and_then(Value::as_f64);
    let limit = plan_usage.get("limit").and_then(Value::as_f64);
    match (remaining, limit) {
        (Some(remaining), Some(limit))
            if remaining.is_finite()
                && limit.is_finite()
                && limit > 0.0
                && remaining >= 0.0
                && remaining <= limit =>
        {
            let used = ((limit - remaining) / limit) * 100.0;
            Ok(round_pair(used))
        }
        _ => Err(
            "Cursor planUsage had no finite remaining percent (totalPercentUsed, or remaining and limit)."
                .to_string(),
        ),
    }
}

fn round_pair(used: f64) -> (f64, f64) {
    let used = (used * 100.0).round() / 100.0;
    let remaining = ((100.0 - used) * 100.0).round() / 100.0;
    (used, remaining)
}

pub(crate) fn billing_reset_iso(value: Option<&Value>) -> Option<String> {
    let value = value?;
    if let Some(text) = value.as_str() {
        let text = text.trim();
        if let Ok(raw) = text.parse::<i64>() {
            return millis_or_secs_iso(raw);
        }
        return DateTime::parse_from_rfc3339(text).ok().map(|parsed| {
            parsed
                .with_timezone(&Utc)
                .to_rfc3339_opts(SecondsFormat::Millis, true)
        });
    }
    value.as_i64().and_then(millis_or_secs_iso)
}

// poteto: GetCurrentPeriodUsage documents unix milliseconds as strings.
// A 10-digit value is treated as seconds.
fn millis_or_secs_iso(raw: i64) -> Option<String> {
    let ms = if raw.abs() >= 1_000_000_000_000 {
        raw
    } else {
        raw.checked_mul(1000)?
    };
    Utc.timestamp_millis_opt(ms)
        .single()
        .map(|instant| instant.to_rfc3339_opts(SecondsFormat::Millis, true))
}

pub(crate) async fn fetch_plan() -> Result<(Option<String>, ParsedPlanWindow), String> {
    let loaded = load_local_auth()?;
    let now = Utc::now().timestamp();
    let access = bearer_for_request(&loaded, now).await?;
    let usage = connect_post(USAGE_URL, &access, "GetCurrentPeriodUsage").await?;
    let plan = if billing_reset_iso(usage.get("billingCycleEnd")).is_none() {
        connect_post(PLAN_URL, &access, "GetPlanInfo").await.ok()
    } else {
        None
    };
    let window = parse_plan_window(&usage, plan.as_ref())?;
    Ok((loaded.email, window))
}

async fn bearer_for_request(auth: &LocalCursorAuth, now_unix: i64) -> Result<String, String> {
    if !needs_refresh(&auth.access_token, now_unix) {
        return Ok(auth.access_token.clone());
    }
    let Some(refresh) = auth.refresh_token.as_deref() else {
        return Err("Cursor access token is expired and no refresh token was stored.".to_string());
    };
    refresh_access_token(refresh).await
}

async fn refresh_access_token(refresh_token: &str) -> Result<String, String> {
    let client = provider_http_client_builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|_| "Cursor token client could not be created.".to_string())?;
    let response = client
        .post(REFRESH_URL)
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "grant_type": "refresh_token",
            "client_id": CLIENT_ID,
            "refresh_token": refresh_token,
        }))
        .send()
        .await
        .map_err(|_| "Cursor token refresh request failed.".to_string())?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|_| "Cursor token refresh body could not be read.".to_string())?;
    if !status.is_success() {
        return Err(format!(
            "Cursor token refresh returned HTTP {}.",
            status.as_u16()
        ));
    }
    let value: Value = serde_json::from_str(&body)
        .map_err(|_| "Cursor token refresh was not JSON.".to_string())?;
    if value
        .get("shouldLogout")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Err("Cursor refresh token was rejected. Sign in again in Cursor.".to_string());
    }
    let access = value
        .get("access_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|token| access_token_usable(token))
        .ok_or_else(|| "Cursor token refresh returned no access token.".to_string())?;
    Ok(access.to_string())
}

async fn connect_post(url: &str, access_token: &str, label: &str) -> Result<Value, String> {
    let client = provider_http_client_builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|_| format!("Cursor {label} client could not be created."))?;
    let response = client
        .post(url)
        .bearer_auth(access_token)
        .header("Content-Type", "application/json")
        .header("Connect-Protocol-Version", "1")
        .body("{}")
        .send()
        .await
        .map_err(|_| format!("Cursor {label} request failed."))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|_| format!("Cursor {label} body could not be read."))?;
    if !status.is_success() {
        return Err(format!("Cursor {label} returned HTTP {}.", status.as_u16()));
    }
    serde_json::from_str(&body).map_err(|_| format!("Cursor {label} was not JSON."))
}

fn load_local_auth() -> Result<LocalCursorAuth, String> {
    let Some(path) = cursor_state_db() else {
        return finish_auth(DbFileState::MissingHome, None, None, None);
    };
    if !path.is_file() {
        return finish_auth(DbFileState::MissingFile, None, None, None);
    }
    let access = match sqlite_value(&path, "cursorAuth/accessToken") {
        Ok(access) => access,
        Err(message) => {
            return match finish_auth(DbFileState::Read, None, None, None) {
                Ok(auth) => Ok(auth),
                Err(_) => Err(message),
            };
        }
    };
    let refresh = sqlite_value(&path, "cursorAuth/refreshToken").unwrap_or(None);
    let email = sqlite_value(&path, "cursorAuth/cachedEmail").unwrap_or(None);
    let state = match access.as_deref() {
        Some(token) if !access_token_usable(token) => DbFileState::Ciphertext,
        _ => DbFileState::Read,
    };
    let access = access.filter(|token| access_token_usable(token));
    finish_auth(state, access, refresh, email)
}

fn finish_auth(
    db_state: DbFileState,
    db_access: Option<String>,
    db_refresh: Option<String>,
    db_email: Option<String>,
) -> Result<LocalCursorAuth, String> {
    let skip_keychain = db_access.as_deref().is_some_and(access_token_usable);
    let (keychain_access, keychain_refresh) = if skip_keychain {
        (None, None)
    } else {
        (
            keychain_secret(ACCESS_SERVICE).ok(),
            keychain_secret(REFRESH_SERVICE).ok(),
        )
    };
    resolve_local_auth(
        db_state,
        db_access.as_deref(),
        db_refresh.as_deref(),
        db_email.as_deref(),
        keychain_access.as_deref(),
        keychain_refresh.as_deref(),
    )
}

fn cursor_state_db() -> Option<PathBuf> {
    // poteto: hardcoded macOS Cursor desktop path. Linux is only a fallback for this VM.
    let home = crate::user_home_dir()?;
    let macos = home.join("Library/Application Support/Cursor/User/globalStorage/state.vscdb");
    if macos.is_file() {
        return Some(macos);
    }
    let linux = home.join(".config/Cursor/User/globalStorage/state.vscdb");
    if linux.is_file() {
        return Some(linux);
    }
    Some(macos)
}

fn sqlite_value(db: &Path, key: &str) -> Result<Option<String>, String> {
    if !matches!(
        key,
        "cursorAuth/accessToken" | "cursorAuth/refreshToken" | "cursorAuth/cachedEmail"
    ) {
        return Err("refused unexpected Cursor state key".to_string());
    }
    let bin = if Path::new("/usr/bin/sqlite3").is_file() {
        PathBuf::from("/usr/bin/sqlite3")
    } else {
        PathBuf::from("sqlite3")
    };
    // poteto: shell out to the sqlite3 that ships on macOS. No rusqlite.
    let output = Command::new(bin)
        .args([
            "-batch",
            "-noheader",
            &sqlite_uri(db),
            &format!("SELECT value FROM ItemTable WHERE key = '{key}';"),
        ])
        .output()
        .map_err(|_| {
            "sqlite3 could not be started. macOS includes /usr/bin/sqlite3.".to_string()
        })?;
    if !output.status.success() {
        return Err("sqlite3 could not read Cursor state.vscdb.".to_string());
    }
    let text = String::from_utf8(output.stdout)
        .map_err(|_| "Cursor state.vscdb value was not UTF-8.".to_string())?;
    let text = text.trim();
    if text.is_empty() {
        Ok(None)
    } else {
        Ok(Some(text.to_string()))
    }
}

pub(crate) fn sqlite_uri(path: &Path) -> String {
    let mut encoded = String::from("file://");
    for byte in path.to_string_lossy().bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' => {
                encoded.push(byte as char);
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded.push_str("?mode=ro");
    encoded
}

fn keychain_secret(service: &str) -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        // poteto: `security` is the macOS keychain reader already used for Claude.
        let output = Command::new("/usr/bin/security")
            .args(["find-generic-password", "-s", service, "-w"])
            .output()
            .map_err(|_| format!("Keychain item {service} could not be read."))?;
        if !output.status.success() {
            return Err(format!("Keychain item {service} was not readable."));
        }
        let text = String::from_utf8(output.stdout)
            .map_err(|_| format!("Keychain item {service} was not UTF-8."))?;
        let text = text.trim();
        if text.is_empty() {
            Err(format!("Keychain item {service} was empty."))
        } else {
            Ok(text.to_string())
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = service;
        Err("Cursor keychain items are only read on macOS.".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn jwt_with_exp(exp: i64) -> String {
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(format!(r#"{{"sub":"user_abc","exp":{exp}}}"#));
        format!("hdr.{payload}.sig")
    }

    #[test]
    fn ciphertext_is_not_a_bearer() {
        assert!(!access_token_usable("v10:not-a-jwt"));
        assert!(access_token_usable(&jwt_with_exp(9_999_999_999)));
    }

    #[test]
    fn expired_jwt_asks_for_refresh_and_a_fresh_one_does_not() {
        let expired = jwt_with_exp(1_000);
        let fresh = jwt_with_exp(9_999_999_999);
        assert!(needs_refresh(&expired, 2_000));
        assert!(!needs_refresh(&fresh, 2_000));
        assert!(!needs_refresh("not-a-jwt", 2_000));
    }

    #[test]
    fn database_jwt_wins_over_keychain_and_email_is_kept() {
        let token = jwt_with_exp(9_999_999_999);
        let auth = resolve_local_auth(
            DbFileState::Read,
            Some(&token),
            Some("refresh-me"),
            Some("dylan@example.com"),
            Some("other-token"),
            None,
        )
        .unwrap();
        assert_eq!(auth.access_token, token);
        assert_eq!(auth.refresh_token.as_deref(), Some("refresh-me"));
        assert_eq!(auth.email.as_deref(), Some("dylan@example.com"));
    }

    #[test]
    fn ciphertext_falls_through_to_a_keychain_jwt() {
        let token = jwt_with_exp(9_999_999_999);
        let auth = resolve_local_auth(
            DbFileState::Ciphertext,
            Some("v10:blob"),
            None,
            Some("not an email"),
            Some(&token),
            Some("k-refresh"),
        )
        .unwrap();
        assert_eq!(auth.access_token, token);
        assert_eq!(auth.refresh_token.as_deref(), Some("k-refresh"));
        assert!(auth.email.is_none());
    }

    #[test]
    fn missing_file_and_missing_keychain_names_the_blocker() {
        let error =
            resolve_local_auth(DbFileState::MissingFile, None, None, None, None, None).unwrap_err();
        assert!(error.contains("state.vscdb"));
        assert!(error.contains("cursor-access-token"));
        assert!(!error.contains("eyJ"));
    }

    #[test]
    fn total_percent_used_becomes_remaining_and_reset() {
        let usage = json!({
            "billingCycleEnd": "1771077734000",
            "planUsage": {
                "remaining": 16778,
                "limit": 40000,
                "totalPercentUsed": 15.48
            }
        });
        let window = parse_plan_window(&usage, None).unwrap();
        assert_eq!(window.used_percent, 15.48);
        assert_eq!(window.remaining_percent, 84.52);
        assert_eq!(
            window.reset_at.as_deref(),
            billing_reset_iso(Some(&json!("1771077734000"))).as_deref()
        );
        let expected = Utc
            .timestamp_millis_opt(1_771_077_734_000)
            .single()
            .unwrap()
            .to_rfc3339_opts(SecondsFormat::Millis, true);
        assert_eq!(window.reset_at.as_deref(), Some(expected.as_str()));
    }

    #[test]
    fn cents_are_used_only_when_percent_is_absent() {
        let usage = json!({
            "planUsage": { "remaining": 16778, "limit": 40000 }
        });
        let plan = json!({
            "planInfo": { "billingCycleEnd": "1771077734000" }
        });
        let window = parse_plan_window(&usage, Some(&plan)).unwrap();
        assert_eq!(window.used_percent, 58.06);
        assert_eq!(window.remaining_percent, 41.94);
        assert!(window.reset_at.is_some());
    }

    #[test]
    fn missing_meters_stay_an_error() {
        let usage = json!({ "planUsage": { "bonusSpend": 0 } });
        let error = parse_plan_window(&usage, None).unwrap_err();
        assert!(error.contains("no finite remaining percent"));
    }

    #[test]
    fn out_of_range_percent_does_not_clamp_into_a_reading() {
        let usage = json!({
            "planUsage": { "totalPercentUsed": 140.0, "remaining": 1, "limit": 0 }
        });
        assert!(parse_plan_window(&usage, None).is_err());
    }

    #[test]
    fn sqlite_uri_encodes_the_application_support_space() {
        let uri = sqlite_uri(Path::new(
            "/Users/dylan/Library/Application Support/Cursor/User/globalStorage/state.vscdb",
        ));
        assert!(uri.starts_with("file:///Users/dylan/Library/Application%20Support/"));
        assert!(uri.ends_with("state.vscdb?mode=ro"));
    }
}
