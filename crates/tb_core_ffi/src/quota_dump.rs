//! Machine-readable snapshot of every quota window TokenBar can see right now.
//!
//! Numbers come from the same provider poll as `tb_agent_usage`. The pace file
//! is not a second source. A provider with no card is an unreadable row.

use crate::agent_cursor::{self, ParsedPlanWindow};
use crate::agent_usage::{AgentDumpCard, AgentDumpWindow};
use serde::Serialize;

const REQUIRED: &[&str] = &["claude", "codex", "grok", "antigravity"];

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DumpFile {
    pub(crate) generated_at: String,
    pub(crate) windows: Vec<DumpWindow>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DumpWindow {
    pub(crate) provider_id: String,
    pub(crate) account: Option<String>,
    pub(crate) account_key: Option<String>,
    pub(crate) window_key: Option<String>,
    pub(crate) remaining_percent: Option<f64>,
    pub(crate) used_percent: Option<f64>,
    pub(crate) reset_at: Option<String>,
    pub(crate) sampled_at: String,
    pub(crate) source: String,
    pub(crate) provider_source: String,
    pub(crate) status: String,
    pub(crate) reason: Option<String>,
}

pub(crate) async fn render(table: bool) -> String {
    let payload = crate::agent_usage::run(0).await;
    let generated_at = payload.generated_at().to_string();
    let cards = payload.dump_cards();
    let labels = payload.opencode_subscriptions().to_vec();
    let sampled_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let cursor = cursor_row(&sampled_at).await;
    let file = assemble(generated_at, cards, cursor, &labels);
    if table {
        format_table(&file)
    } else {
        serde_json::to_string_pretty(&file).unwrap_or_else(|error| {
            format!(r#"{{"ok":false,"err":"quota dump serialization failed: {error}"}}"#)
        })
    }
}

async fn cursor_row(sampled_at: &str) -> DumpWindow {
    match agent_cursor::fetch_plan().await {
        Ok((account, window)) => readable_cursor(account, window, sampled_at),
        Err(reason) => unreadable(
            "cursor",
            None,
            None,
            Some(agent_cursor::WINDOW_KEY.to_string()),
            sampled_at,
            "dashboard",
            reason,
        ),
    }
}

fn readable_cursor(
    account: Option<String>,
    window: ParsedPlanWindow,
    sampled_at: &str,
) -> DumpWindow {
    DumpWindow {
        provider_id: "cursor".to_string(),
        account,
        account_key: None,
        window_key: Some(agent_cursor::WINDOW_KEY.to_string()),
        remaining_percent: Some(window.remaining_percent),
        used_percent: Some(window.used_percent),
        reset_at: window.reset_at,
        sampled_at: sampled_at.to_string(),
        source: "live".to_string(),
        provider_source: "dashboard".to_string(),
        status: "readable".to_string(),
        reason: None,
    }
}

pub(crate) fn assemble(
    generated_at: String,
    cards: Vec<AgentDumpCard>,
    cursor: DumpWindow,
    opencode_labels: &[String],
) -> DumpFile {
    let mut windows = Vec::new();
    for card in cards {
        windows.extend(rows_from_card(&card));
    }
    for provider_id in REQUIRED {
        if !windows.iter().any(|row| row.provider_id == *provider_id) {
            windows.push(missing_provider(provider_id, &generated_at));
        }
    }
    if !windows.iter().any(|row| row.provider_id == "cursor") {
        windows.push(cursor);
    }
    if !windows.iter().any(|row| row.provider_id == "opencode") {
        windows.push(opencode_row(opencode_labels, &generated_at));
    }
    if !windows.iter().any(|row| row.provider_id == "pi") {
        windows.push(unreadable(
            "pi",
            None,
            None,
            None,
            &generated_at,
            "none",
            "Pi has no quota fetch in TokenBar. Local session logs are not a plan window."
                .to_string(),
        ));
    }
    windows.sort_by_key(|row| provider_rank(&row.provider_id));
    DumpFile {
        generated_at,
        windows,
    }
}

fn rows_from_card(card: &AgentDumpCard) -> Vec<DumpWindow> {
    if let Some(reason) = card.error.as_ref() {
        if card.windows.is_empty() {
            return vec![unreadable(
                &card.provider_id,
                card.account.clone(),
                card.account_key.clone(),
                None,
                &card.sampled_at,
                &card.provider_source,
                reason.clone(),
            )];
        }
        return card
            .windows
            .iter()
            .map(|window| {
                window_row(
                    card,
                    window,
                    "unreadable",
                    "last-good",
                    Some(reason.clone()),
                )
            })
            .collect();
    }
    if card.windows.is_empty() {
        return vec![unreadable(
            &card.provider_id,
            card.account.clone(),
            card.account_key.clone(),
            None,
            &card.sampled_at,
            &card.provider_source,
            format!("{} returned no quota windows.", card.provider_id),
        )];
    }
    card.windows
        .iter()
        .map(|window| {
            if percent_finite(window) {
                window_row(card, window, "readable", "live", None)
            } else {
                window_row(
                    card,
                    window,
                    "unreadable",
                    "live",
                    Some("window percent was not finite".to_string()),
                )
            }
        })
        .collect()
}

fn percent_finite(window: &AgentDumpWindow) -> bool {
    window.remaining_percent.is_finite() && window.used_percent.is_finite()
}

fn window_row(
    card: &AgentDumpCard,
    window: &AgentDumpWindow,
    status: &str,
    source: &str,
    reason: Option<String>,
) -> DumpWindow {
    let readable = status == "readable";
    DumpWindow {
        provider_id: card.provider_id.clone(),
        account: card.account.clone(),
        account_key: card.account_key.clone(),
        window_key: window.window_key.clone(),
        remaining_percent: readable.then_some(window.remaining_percent),
        used_percent: readable.then_some(window.used_percent),
        reset_at: window.reset_at.clone(),
        sampled_at: card.sampled_at.clone(),
        source: source.to_string(),
        provider_source: card.provider_source.clone(),
        status: status.to_string(),
        reason,
    }
}

fn missing_provider(provider_id: &str, sampled_at: &str) -> DumpWindow {
    let reason = match provider_id {
        "grok" => "Grok returned no quota card. TokenBar only fetches Grok when ~/.grok/auth.json has credentials.",
        "claude" => "Claude returned no quota card.",
        "codex" => "Codex returned no quota card.",
        "antigravity" => "Antigravity returned no quota card.",
        _ => "TokenBar returned no quota card for this provider.",
    };
    unreadable(
        provider_id,
        None,
        None,
        None,
        sampled_at,
        "none",
        reason.to_string(),
    )
}

fn opencode_row(labels: &[String], sampled_at: &str) -> DumpWindow {
    let reason = if labels.is_empty() {
        "OpenCode has no quota window in TokenBar. auth.json had no oauth subscription labels."
            .to_string()
    } else {
        format!(
            "OpenCode has no quota window in TokenBar. OAuth labels are not remaining percent: {}.",
            labels.join(", ")
        )
    };
    unreadable("opencode", None, None, None, sampled_at, "none", reason)
}

fn unreadable(
    provider_id: &str,
    account: Option<String>,
    account_key: Option<String>,
    window_key: Option<String>,
    sampled_at: &str,
    provider_source: &str,
    reason: String,
) -> DumpWindow {
    DumpWindow {
        provider_id: provider_id.to_string(),
        account,
        account_key,
        window_key,
        remaining_percent: None,
        used_percent: None,
        reset_at: None,
        sampled_at: sampled_at.to_string(),
        source: "live".to_string(),
        provider_source: provider_source.to_string(),
        status: "unreadable".to_string(),
        reason: Some(reason),
    }
}

fn provider_rank(provider_id: &str) -> u8 {
    match provider_id {
        "claude" => 0,
        "codex" => 1,
        "grok" => 2,
        "antigravity" => 3,
        "cursor" => 4,
        "opencode" => 5,
        "pi" => 6,
        "copilot" => 7,
        _ => 8,
    }
}

pub(crate) fn format_table(file: &DumpFile) -> String {
    let mut lines = vec![
        format!("generatedAt {}", file.generated_at),
        format!(
            "{:<14} {:<12} {:>11}  {:<28} {:<18} {}",
            "providerId", "status", "remaining%", "resetAt", "windowKey", "reason"
        ),
    ];
    for window in &file.windows {
        let remaining = window
            .remaining_percent
            .map(|value| format!("{value:.2}"))
            .unwrap_or_else(|| "-".to_string());
        let reason = window.reason.as_deref().unwrap_or("-");
        lines.push(format!(
            "{:<14} {:<12} {:>11}  {:<28} {:<18} {}",
            window.provider_id,
            window.status,
            remaining,
            window.reset_at.as_deref().unwrap_or("-"),
            window.window_key.as_deref().unwrap_or("-"),
            clip(reason, 96),
        ));
    }
    lines.join("\n")
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut clipped: String = text.chars().take(max.saturating_sub(3)).collect();
    clipped.push_str("...");
    clipped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(
        provider_id: &str,
        error: Option<&str>,
        windows: Vec<AgentDumpWindow>,
    ) -> AgentDumpCard {
        AgentDumpCard {
            provider_id: provider_id.to_string(),
            account: Some("dylan@example.com".to_string()),
            account_key: None,
            provider_source: "oauth".to_string(),
            sampled_at: "2026-10-03T17:00:00.000Z".to_string(),
            error: error.map(str::to_string),
            windows,
        }
    }

    fn window(key: &str, remaining: f64) -> AgentDumpWindow {
        AgentDumpWindow {
            window_key: Some(key.to_string()),
            remaining_percent: remaining,
            used_percent: 100.0 - remaining,
            reset_at: Some("2026-10-03T22:00:00.000Z".to_string()),
        }
    }

    fn cursor_down() -> DumpWindow {
        unreadable(
            "cursor",
            None,
            None,
            Some("plan.period.v1".to_string()),
            "2026-10-03T17:00:00.000Z",
            "dashboard",
            "Cursor state.vscdb is missing, and keychain item cursor-access-token was not readable."
                .to_string(),
        )
    }

    #[test]
    fn live_windows_keep_their_keys_and_gaps_stay_unreadable() {
        let cards = vec![
            card("claude", None, vec![window("five_hour.v1", 80.0)]),
            card("codex", None, vec![window("weekly.v1", 40.0)]),
            card(
                "antigravity",
                Some("Antigravity IDE was not running."),
                vec![],
            ),
        ];
        let file = assemble(
            "2026-10-03T17:00:00.000Z".to_string(),
            cards,
            cursor_down(),
            &["Codex".to_string()],
        );
        let json = serde_json::to_value(&file).unwrap();
        let windows = json["windows"].as_array().unwrap();
        let claude = windows
            .iter()
            .find(|row| row["providerId"] == "claude")
            .unwrap();
        assert_eq!(claude["status"], "readable");
        assert_eq!(claude["source"], "live");
        assert_eq!(claude["windowKey"], "five_hour.v1");
        assert_eq!(claude["remainingPercent"], 80.0);
        assert_eq!(claude["usedPercent"], 20.0);
        assert_eq!(claude["resetAt"], "2026-10-03T22:00:00.000Z");
        assert_eq!(claude["sampledAt"], "2026-10-03T17:00:00.000Z");
        assert_eq!(claude["account"], "dylan@example.com");

        let antigravity = windows
            .iter()
            .find(|row| row["providerId"] == "antigravity")
            .unwrap();
        assert_eq!(antigravity["status"], "unreadable");
        assert!(antigravity["reason"]
            .as_str()
            .unwrap()
            .contains("not running"));
        assert!(antigravity["remainingPercent"].is_null());

        for provider in ["grok", "cursor", "opencode", "pi"] {
            let row = windows
                .iter()
                .find(|row| row["providerId"] == provider)
                .unwrap_or_else(|| panic!("{provider} omitted"));
            assert_eq!(row["status"], "unreadable", "{provider}");
            assert!(row["remainingPercent"].is_null(), "{provider}");
            assert!(row["reason"].as_str().unwrap().len() > 8, "{provider}");
        }
        let opencode = windows
            .iter()
            .find(|row| row["providerId"] == "opencode")
            .unwrap();
        assert!(opencode["reason"].as_str().unwrap().contains("Codex"));
        assert!(!opencode["reason"].as_str().unwrap().contains('%'));
    }

    #[test]
    fn last_good_windows_are_not_labeled_live() {
        let cards = vec![card(
            "claude",
            Some("Claude usage timed out."),
            vec![window("five_hour.v1", 10.0)],
        )];
        let file = assemble("t".to_string(), cards, cursor_down(), &[]);
        let claude = file
            .windows
            .iter()
            .find(|row| row.provider_id == "claude")
            .unwrap();
        assert_eq!(claude.status, "unreadable");
        assert_eq!(claude.source, "last-good");
        assert_eq!(claude.remaining_percent, None);
        assert_eq!(claude.reset_at.as_deref(), Some("2026-10-03T22:00:00.000Z"));
    }

    #[test]
    fn table_names_every_required_provider() {
        let file = assemble("t".to_string(), vec![], cursor_down(), &[]);
        let table = format_table(&file);
        for provider in [
            "claude",
            "codex",
            "grok",
            "antigravity",
            "cursor",
            "opencode",
            "pi",
        ] {
            assert!(table.contains(provider), "{table}");
            assert!(table.contains("unreadable"));
        }
    }
}
