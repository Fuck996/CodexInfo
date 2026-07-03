use chrono::{DateTime, Datelike, Duration as ChronoDuration, Local, NaiveDate, TimeZone, Utc};
use reqwest::{blocking::Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    cmp::Reverse,
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
        Arc, Mutex, OnceLock,
    },
    thread,
    time::Duration,
};
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
use tauri::{
    image::Image,
    menu::{CheckMenuItem, Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    window::{ProgressBarState, ProgressBarStatus},
    AppHandle, Manager, Runtime, WebviewWindow,
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tauri_plugin_window_state::{AppHandleExt, StateFlags};
use walkdir::WalkDir;
#[cfg(target_os = "windows")]
use windows::{
    core::{w, PCWSTR},
    Win32::{
        Foundation::{HWND, RECT},
        UI::Accessibility::{SetWinEventHook, HWINEVENTHOOK},
        UI::WindowsAndMessaging::{
            FindWindowExW, FindWindowW, GetClassNameW, GetMessageW, GetWindowRect, SetWindowPos,
            EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_SHOW, HWND_TOP, HWND_TOPMOST, MSG,
            SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, WINEVENT_OUTOFCONTEXT,
        },
    },
};

const COLLAPSED_WIDTH: f64 = 460.0;
const COLLAPSED_HEIGHT: f64 = 430.0;
const MAX_EXPANDED_HEIGHT: f64 = 920.0;
const DOCK_WIDTH: f64 = 250.0;
const DOCK_HEIGHT: f64 = 42.0;
const DOCK_TRAY_GAP: f64 = 12.0;
const DOCK_EDGE_GAP: f64 = 4.0;
const APP_SERVER_TIMEOUT_MS: u64 = 8_000;
const MAX_SESSION_FILES_TO_SCAN: usize = 120;
const CHATGPT_BACKEND_URL: &str = "https://chatgpt.com/backend-api";
const OPENAI_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
#[cfg(target_os = "windows")]
static DOCK_Z_ORDER_EVENTS: OnceLock<mpsc::Sender<()>> = OnceLock::new();

#[derive(Clone)]
struct AppState {
    keep_always_on_top: Arc<AtomicBool>,
    tray_usage_enabled: Arc<AtomicBool>,
    dock_enabled: Arc<AtomicBool>,
    taskbar_usage_enabled: Arc<AtomicBool>,
    main_hovered: Arc<AtomicBool>,
    dock_hovered: Arc<AtomicBool>,
    hover_component_visible: Arc<AtomicBool>,
    hover_monitor_running: Arc<AtomicBool>,
    dock_pending_position: Arc<Mutex<Option<(i32, i32)>>>,
    latest_tray_usage: Arc<Mutex<Option<TrayUsageText>>>,
}

#[derive(Debug, Clone)]
struct TrayUsageText {
    title: String,
    tooltip: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageWindow {
    id: String,
    label: String,
    used: f64,
    total: f64,
    reset_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_mode: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct TokenUsage {
    input: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cached_input: Option<f64>,
    output: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_output: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    total: Option<f64>,
    limit: Option<f64>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct TokenPeriodUsage {
    id: String,
    label: String,
    range_label: String,
    token_usage: TokenUsage,
    computed_at: String,
    has_data: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ResetChance {
    id: String,
    label: String,
    expires_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResetCredits {
    available: u32,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageSnapshot {
    plan_name: String,
    subscription_expires_at: String,
    windows: Vec<UsageWindow>,
    token_usage: TokenUsage,
    credits_remaining: f64,
    credits_total: f64,
    resets: Vec<ResetChance>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reset_credits: Option<ResetCredits>,
    updated_at: String,
    source_path: String,
    account_source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cached_at: Option<String>,
    #[serde(default)]
    token_periods: Vec<TokenPeriodUsage>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AppSettings {
    launch_at_startup: bool,
    main_visible: bool,
    keep_always_on_top: bool,
    tray_usage_enabled: bool,
    dock_enabled: bool,
    taskbar_usage_enabled: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(default)]
struct StoredSettings {
    main_visible: bool,
    keep_always_on_top: bool,
    tray_usage_enabled: bool,
    dock_enabled: bool,
    taskbar_usage_enabled: bool,
}

impl Default for StoredSettings {
    fn default() -> Self {
        Self {
            main_visible: false,
            keep_always_on_top: true,
            tray_usage_enabled: true,
            dock_enabled: true,
            taskbar_usage_enabled: false,
        }
    }
}

#[derive(Debug)]
struct DiscoveredSnapshot {
    snapshot: UsageSnapshot,
    source_path: String,
    account_source: String,
}

#[derive(Debug, Clone)]
struct TokenCountEvent {
    timestamp: String,
    payload: Value,
    source_path: PathBuf,
}

fn codex_root() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".codex"))
}

fn sessions_path() -> Option<PathBuf> {
    codex_root().map(|root| root.join("sessions"))
}

fn auth_path() -> Option<PathBuf> {
    codex_root().map(|root| root.join("auth.json"))
}

fn usage_cache_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_data_dir().map_err(|error| error.to_string())?;
    fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    Ok(dir.join("usage-cache.json"))
}

fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_data_dir().map_err(|error| error.to_string())?;
    fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    Ok(dir.join("settings.json"))
}

fn read_stored_settings(app: &AppHandle) -> Result<StoredSettings, String> {
    let path = settings_path(app)?;
    if !path.exists() {
        return Ok(StoredSettings::default());
    }
    let content = fs::read_to_string(&path).map_err(|error| error.to_string())?;
    serde_json::from_str::<StoredSettings>(&content).map_err(|error| error.to_string())
}

fn write_stored_settings(app: &AppHandle, settings: &StoredSettings) -> Result<(), String> {
    let path = settings_path(app)?;
    let content = serde_json::to_string_pretty(settings).map_err(|error| error.to_string())?;
    fs::write(path, content).map_err(|error| error.to_string())
}

fn plan_label(plan_type: &str) -> String {
    let normalized = plan_type
        .trim()
        .to_ascii_lowercase()
        .replace('_', "-")
        .replace(' ', "-");
    match normalized.as_str() {
        "free" | "chatgpt-free" => "Free".to_string(),
        "plus" | "chatgpt-plus" => "Plus".to_string(),
        "prolite" | "pro-lite" | "pro-5x" | "codex-pro-lite" | "codex-pro-5x" => "Pro 5X".to_string(),
        "pro" | "promax" | "pro-max" | "pro-20x" | "chatgpt-pro" | "codex-pro-20x" => {
            "Pro 20X".to_string()
        }
        "team" | "chatgpt-team" => "Team".to_string(),
        "business" | "chatgpt-business" => "Business".to_string(),
        "enterprise" | "chatgpt-enterprise" => "Enterprise".to_string(),
        "edu" | "chatgpt-edu" => "Edu".to_string(),
        _ => plan_type.trim().to_string(),
    }
}

fn number(value: &Value) -> Option<f64> {
    value.as_f64().filter(|value| value.is_finite())
}

fn string_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn unix_seconds_to_iso(value: f64) -> String {
    DateTime::<Utc>::from_timestamp(value as i64, 0)
        .unwrap_or_else(Utc::now)
        .to_rfc3339()
}

fn app_server_window_to_usage(id: &str, label: &str, value: Option<&Value>) -> Option<UsageWindow> {
    let window = value?;
    let used_percent = number(window.get("usedPercent")?)?;
    let resets_at = number(window.get("resetsAt")?)?;

    Some(UsageWindow {
        id: id.to_string(),
        label: label.to_string(),
        used: used_percent.clamp(0.0, 100.0),
        total: 100.0,
        reset_at: unix_seconds_to_iso(resets_at),
        display_mode: Some("percent".to_string()),
    })
}

fn session_window_to_usage(id: &str, label: &str, value: Option<&Value>) -> Option<UsageWindow> {
    let window = value?;
    let used_percent = number(window.get("used_percent")?)?;
    let resets_at = number(window.get("resets_at")?)?;

    Some(UsageWindow {
        id: id.to_string(),
        label: label.to_string(),
        used: used_percent.clamp(0.0, 100.0),
        total: 100.0,
        reset_at: unix_seconds_to_iso(resets_at),
        display_mode: Some("percent".to_string()),
    })
}

fn token_usage_from_session(event: Option<&TokenCountEvent>) -> Option<TokenUsage> {
    let payload = &event?.payload;
    let stats = payload.get("info")?.get("total_token_usage")?;
    let input = stats.get("input_tokens").and_then(number).unwrap_or(0.0);
    let cached_input = stats.get("cached_input_tokens").and_then(number).unwrap_or(0.0);
    let output = stats.get("output_tokens").and_then(number).unwrap_or(0.0);
    let reasoning_output = stats.get("reasoning_output_tokens").and_then(number).unwrap_or(0.0);
    let output_total = output + reasoning_output;
    let total = stats.get("total_tokens").and_then(number).unwrap_or(input + output_total);

    Some(TokenUsage {
        input: Some(input),
        cached_input: Some(cached_input),
        output: Some(output_total),
        reasoning_output: Some(reasoning_output),
        total: Some(total),
        limit: None,
    })
}

fn empty_token_usage() -> TokenUsage {
    TokenUsage {
        input: Some(0.0),
        cached_input: Some(0.0),
        output: Some(0.0),
        reasoning_output: Some(0.0),
        total: Some(0.0),
        limit: None,
    }
}

fn merge_token_usage(total: &mut TokenUsage, usage: &TokenUsage) {
    total.input = Some(total.input.unwrap_or(0.0) + usage.input.unwrap_or(0.0));
    total.cached_input = Some(total.cached_input.unwrap_or(0.0) + usage.cached_input.unwrap_or(0.0));
    total.output = Some(total.output.unwrap_or(0.0) + usage.output.unwrap_or(0.0));
    total.reasoning_output = Some(total.reasoning_output.unwrap_or(0.0) + usage.reasoning_output.unwrap_or(0.0));
    total.total = Some(total.total.unwrap_or(0.0) + usage.total.unwrap_or(0.0));
}

fn token_event_time(event: &TokenCountEvent) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&event.timestamp)
        .ok()
        .map(|time| time.with_timezone(&Utc))
}

fn local_midnight_utc(date: NaiveDate) -> Option<DateTime<Utc>> {
    let midnight = date.and_hms_opt(0, 0, 0)?;
    Local
        .from_local_datetime(&midnight)
        .earliest()
        .map(|time| time.with_timezone(&Utc))
}

fn add_months(year: i32, month: u32, offset: i32) -> Option<NaiveDate> {
    let zero_based = year * 12 + month as i32 - 1 + offset;
    let next_year = zero_based.div_euclid(12);
    let next_month = zero_based.rem_euclid(12) as u32 + 1;
    NaiveDate::from_ymd_opt(next_year, next_month, 1)
}

fn format_period_range(start: DateTime<Utc>, end: DateTime<Utc>) -> String {
    let end_inclusive = end - ChronoDuration::seconds(1);
    format!(
        "{} · {}",
        start.with_timezone(&Local).format("%Y-%m-%d"),
        end_inclusive.with_timezone(&Local).format("%Y-%m-%d")
    )
}

fn token_period_ranges() -> Vec<(&'static str, &'static str, DateTime<Utc>, DateTime<Utc>)> {
    let today = Local::now().date_naive();
    let week_start = today - ChronoDuration::days(today.weekday().num_days_from_monday() as i64);
    let this_month_start = NaiveDate::from_ymd_opt(today.year(), today.month(), 1)
        .unwrap_or(today);
    let last_month_start = add_months(this_month_start.year(), this_month_start.month(), -1)
        .unwrap_or(this_month_start);
    let next_month_start = add_months(this_month_start.year(), this_month_start.month(), 1)
        .unwrap_or(this_month_start);

    [
        ("thisWeek", "本周", week_start, week_start + ChronoDuration::days(7)),
        ("lastWeek", "上周", week_start - ChronoDuration::days(7), week_start),
        ("thisMonth", "本月", this_month_start, next_month_start),
        ("lastMonth", "上月", last_month_start, this_month_start),
    ]
    .into_iter()
    .filter_map(|(id, label, start, end)| Some((id, label, local_midnight_utc(start)?, local_midnight_utc(end)?)))
    .collect()
}

fn build_token_periods(events: &[TokenCountEvent]) -> Vec<TokenPeriodUsage> {
    token_period_ranges()
        .into_iter()
        .map(|(id, label, start, end)| {
            let mut token_usage = empty_token_usage();
            let mut latest_time: Option<DateTime<Utc>> = None;
            let mut has_data = false;

            for event in events {
                let Some(event_time) = token_event_time(event) else {
                    continue;
                };
                if event_time < start || event_time >= end {
                    continue;
                }
                let Some(usage) = token_usage_from_session(Some(event)) else {
                    continue;
                };
                merge_token_usage(&mut token_usage, &usage);
                latest_time = Some(latest_time.map_or(event_time, |latest| latest.max(event_time)));
                has_data = true;
            }

            TokenPeriodUsage {
                id: id.to_string(),
                label: label.to_string(),
                range_label: format_period_range(start, end),
                token_usage,
                computed_at: latest_time.unwrap_or_else(Utc::now).to_rfc3339(),
                has_data,
            }
        })
        .collect()
}

fn snapshot_from_token_count(event: &TokenCountEvent) -> Result<UsageSnapshot, String> {
    let rate_limits = event
        .payload
        .get("rate_limits")
        .ok_or_else(|| format!("Codex 会话日志缺少 rate_limits: {}", event.source_path.display()))?;
    let mut windows = Vec::new();
    if let Some(window) = session_window_to_usage("fiveHour", "5 小时用量", rate_limits.get("primary")) {
        windows.push(window);
    }
    if let Some(window) = session_window_to_usage("sevenDay", "7 天用量", rate_limits.get("secondary")) {
        windows.push(window);
    }
    if windows.is_empty() {
        return Err(format!(
            "Codex 会话日志缺少用量窗口: {}",
            event.source_path.display()
        ));
    }

    let plan_name = string_field(rate_limits, "plan_type")
        .map(plan_label)
        .unwrap_or_else(|| "Codex".to_string());

    Ok(UsageSnapshot {
        plan_name,
        subscription_expires_at: String::new(),
        windows,
        token_usage: token_usage_from_session(Some(event)).unwrap_or(TokenUsage {
            input: None,
            cached_input: None,
            output: None,
            reasoning_output: None,
            total: None,
            limit: None,
        }),
        credits_remaining: 0.0,
        credits_total: 0.0,
        resets: Vec::new(),
        reset_credits: None,
        updated_at: event.timestamp.clone(),
        source_path: event.source_path.display().to_string(),
        account_source: "usageFile".to_string(),
        cached_at: None,
        token_periods: Vec::new(),
    })
}

fn parse_token_count_line(line: &str, source_path: &Path) -> Option<TokenCountEvent> {
    if !line.contains("\"token_count\"") || !line.contains("\"rate_limits\"") {
        return None;
    }

    let parsed: Value = serde_json::from_str(line).ok()?;
    if parsed.get("payload")?.get("type")?.as_str()? != "token_count" {
        return None;
    }

    Some(TokenCountEvent {
        timestamp: parsed.get("timestamp")?.as_str()?.to_string(),
        payload: parsed.get("payload")?.clone(),
        source_path: source_path.to_path_buf(),
    })
}

fn latest_token_count_in_file(path: &Path) -> Option<TokenCountEvent> {
    let content = fs::read_to_string(path).ok()?;
    for line in content.lines().rev() {
        if let Some(event) = parse_token_count_line(line, path) {
            return Some(event);
        }
    }
    None
}

fn find_latest_token_counts() -> Vec<TokenCountEvent> {
    let Some(root) = sessions_path() else {
        return Vec::new();
    };
    if !root.exists() {
        return Vec::new();
    }

    let mut candidates: Vec<_> = WalkDir::new(root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("jsonl"))
        .filter_map(|entry| {
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((entry.path().to_path_buf(), modified))
        })
        .collect();

    candidates.sort_by_key(|(_, modified)| Reverse(*modified));

    candidates
        .into_iter()
        .take(MAX_SESSION_FILES_TO_SCAN)
        .filter_map(|(path, _)| latest_token_count_in_file(&path))
        .collect()
}

fn find_latest_token_count(events: &[TokenCountEvent]) -> Option<TokenCountEvent> {
    events
        .iter()
        .max_by_key(|event| event.timestamp.as_str())
        .cloned()
}

fn find_codex_cli_path() -> Option<PathBuf> {
    let local_app_data = std::env::var_os("LOCALAPPDATA").map(PathBuf::from)?;
    let direct = local_app_data.join("OpenAI").join("Codex").join("bin").join("codex.exe");
    if direct.exists() {
        return Some(direct);
    }

    let bin_root = local_app_data.join("OpenAI").join("Codex").join("bin");
    let mut nested = Vec::new();
    for entry in fs::read_dir(bin_root).ok()? {
        let entry = entry.ok()?;
        if !entry.file_type().ok()?.is_dir() {
            continue;
        }
        let candidate = entry.path().join("codex.exe");
        if candidate.exists() {
            let modified = fs::metadata(&candidate).ok()?.modified().ok()?;
            nested.push((candidate, modified));
        }
    }
    nested.sort_by_key(|(_, modified)| Reverse(*modified));
    nested.into_iter().map(|(path, _)| path).next()
}

fn read_codex_app_server() -> Option<Value> {
    let codex_path = find_codex_cli_path()?;
    let mut child = Command::new(codex_path)
        .args(["app-server", "--listen", "stdio://"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(0x08000000)
        .spawn()
        .ok()?;

    let mut stdin = child.stdin.take()?;
    let stdout = child.stdout.take()?;
    let (tx, rx) = mpsc::channel::<Value>();

    thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines().map_while(Result::ok) {
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(value) = serde_json::from_str::<Value>(&line) {
                let _ = tx.send(value);
            }
        }
    });

    let messages = [
        json!({
            "method": "initialize",
            "id": 1,
            "params": {
                "clientInfo": {
                    "name": "codex_info_widget",
                    "title": "CodexInfo",
                    "version": "0.1.0"
                },
                "capabilities": {
                    "experimentalApi": true
                }
            }
        }),
        json!({"method": "initialized", "params": {}}),
        json!({"method": "account/read", "id": 2, "params": {"refreshToken": false}}),
        json!({"method": "account/rateLimits/read", "id": 3}),
    ];

    for message in messages {
        let _ = writeln!(stdin, "{message}");
    }

    let mut account: Option<Value> = None;
    let mut rate_limits: Option<Value> = None;
    let deadline = Duration::from_millis(APP_SERVER_TIMEOUT_MS);
    let started = std::time::Instant::now();

    while started.elapsed() < deadline {
        let remaining = deadline.saturating_sub(started.elapsed());
        let Ok(message) = rx.recv_timeout(remaining.min(Duration::from_millis(500))) else {
            continue;
        };

        match message.get("id").and_then(Value::as_i64) {
            Some(2) => account = message.get("result").cloned(),
            Some(3) => rate_limits = message.get("result").cloned(),
            _ => {}
        }

        if account.is_some() && rate_limits.is_some() {
            let _ = child.kill();
            return Some(json!({
                "account": account,
                "rateLimits": rate_limits,
                "sourcePath": "codex app-server"
            }));
        }
    }

    let _ = child.kill();
    rate_limits.map(|rate_limits| {
        json!({
            "account": account,
            "rateLimits": rate_limits,
            "sourcePath": "codex app-server"
        })
    })
}

fn pick_codex_bucket(rate_limits: &Value) -> Option<&Value> {
    rate_limits
        .get("rateLimitsByLimitId")
        .and_then(|buckets| buckets.get("codex"))
        .or_else(|| rate_limits.get("rateLimits"))
}

fn extract_reset_expires(value: &Value) -> Option<String> {
    for key in [
        "expiresAt",
        "expires_at",
        "expirationTime",
        "expiration_time",
        "expires",
        "expiresOn",
    ] {
        if let Some(text) = string_field(value, key) {
            return Some(text.to_string());
        }
        if let Some(seconds) = value.get(key).and_then(number) {
            return Some(unix_seconds_to_iso(seconds));
        }
    }
    None
}

fn extract_reset_list(value: Option<&Value>) -> Vec<ResetChance> {
    let Some(value) = value else {
        return Vec::new();
    };

    let arrays = ["resets", "items", "credits", "available"];
    for key in arrays {
        let Some(list) = value.get(key).and_then(Value::as_array) else {
            continue;
        };
        let resets: Vec<_> = list
            .iter()
            .enumerate()
            .filter_map(|(index, item)| {
                let expires_at = extract_reset_expires(item)?;
                Some(ResetChance {
                    id: string_field(item, "id")
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("reset-{}", index + 1)),
                    label: format!("重置机会 {:02}", index + 1),
                    expires_at,
                })
            })
            .collect();
        if !resets.is_empty() {
            return resets;
        }
    }

    Vec::new()
}

fn extract_reset_count(value: Option<&Value>, reset_list_len: usize) -> Option<ResetCredits> {
    let value = value?;
    let available = value
        .get("availableCount")
        .and_then(number)
        .or_else(|| value.get("available_count").and_then(number))
        .or_else(|| value.get("available").and_then(number))
        .or_else(|| value.get("count").and_then(number))
        .or_else(|| value.get("remaining").and_then(number));

    available.map(|count| ResetCredits {
        available: count.max(reset_list_len as f64).floor() as u32,
    })
}

fn auth_token<'a>(auth: &'a Value, key: &str) -> Option<&'a str> {
    auth.get("tokens")
        .and_then(|tokens| tokens.get(key))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
}

fn load_codex_auth() -> Result<(PathBuf, Value), String> {
    let path = auth_path().ok_or_else(|| "无法定位用户目录，不能读取 Codex 登录信息。".to_string())?;
    let content = fs::read_to_string(&path)
        .map_err(|error| format!("无法读取 Codex 登录文件 {}: {error}", path.display()))?;
    let auth = serde_json::from_str::<Value>(&content)
        .map_err(|error| format!("Codex 登录文件格式无效 {}: {error}", path.display()))?;
    Ok((path, auth))
}

fn save_codex_auth(path: &Path, auth: &Value) -> Result<(), String> {
    let temporary = path.with_file_name(format!(
        ".auth.json.tmp-{}",
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    let content = serde_json::to_string_pretty(auth).map_err(|error| error.to_string())?;
    fs::write(&temporary, content).map_err(|error| format!("无法写入刷新后的 Codex 登录文件: {error}"))?;
    fs::rename(&temporary, path).map_err(|error| format!("无法替换 Codex 登录文件: {error}"))
}

fn refresh_codex_auth(client: &Client, path: &Path, auth: &mut Value) -> Result<(), String> {
    let refresh_token = auth_token(auth, "refresh_token")
        .ok_or_else(|| "Codex 登录文件缺少 refresh_token，无法刷新访问令牌。".to_string())?
        .to_string();
    let response = client
        .post(OPENAI_TOKEN_URL)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token.as_str()),
        ])
        .send()
        .map_err(|error| format!("刷新 Codex 访问令牌失败: {error}"))?;

    if !response.status().is_success() {
        return Err(format!("刷新 Codex 访问令牌失败，HTTP {}", response.status()));
    }

    let body = response
        .json::<Value>()
        .map_err(|error| format!("刷新令牌响应不是有效 JSON: {error}"))?;
    let access_token = string_field(&body, "access_token")
        .ok_or_else(|| "刷新令牌响应缺少 access_token。".to_string())?;

    let tokens = auth
        .get_mut("tokens")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| "Codex 登录文件缺少 tokens 对象。".to_string())?;
    tokens.insert("access_token".to_string(), Value::String(access_token.to_string()));
    if let Some(id_token) = string_field(&body, "id_token") {
        tokens.insert("id_token".to_string(), Value::String(id_token.to_string()));
    }
    if let Some(next_refresh_token) = string_field(&body, "refresh_token") {
        tokens.insert(
            "refresh_token".to_string(),
            Value::String(next_refresh_token.to_string()),
        );
    }
    auth["last_refresh"] = Value::String(Utc::now().to_rfc3339());
    save_codex_auth(path, auth)
}

fn request_chatgpt_json(client: &Client, auth: &Value, path: &str) -> Result<Value, String> {
    let access_token = auth_token(auth, "access_token")
        .ok_or_else(|| "Codex 登录文件缺少 access_token。".to_string())?;
    let url = format!("{CHATGPT_BACKEND_URL}/{path}");
    let mut request = client
        .get(url)
        .bearer_auth(access_token)
        .header("Accept", "application/json");
    if let Some(account_id) = auth_token(auth, "account_id") {
        request = request.header("ChatGPT-Account-Id", account_id);
    }

    let response = request
        .send()
        .map_err(|error| format!("请求 ChatGPT 用量接口失败: {error}"))?;
    let status = response.status();
    if status == StatusCode::UNAUTHORIZED {
        return Err("unauthorized".to_string());
    }
    if !status.is_success() {
        return Err(format!("ChatGPT 用量接口返回 HTTP {status}"));
    }
    response
        .json::<Value>()
        .map_err(|error| format!("ChatGPT 用量接口响应不是有效 JSON: {error}"))
}

fn reset_details_from_chatgpt_response(value: &Value) -> (Option<ResetCredits>, Vec<ResetChance>) {
    let credits_source = value
        .as_array()
        .cloned()
        .or_else(|| value.get("credits").and_then(Value::as_array).cloned())
        .unwrap_or_default();
    let resets: Vec<_> = credits_source
        .iter()
        .filter(|item| {
            string_field(item, "status")
                .map(|status| status.eq_ignore_ascii_case("available"))
                .unwrap_or(true)
        })
        .enumerate()
        .filter_map(|(index, item)| {
            let expires_at = extract_reset_expires(item)?;
            Some(ResetChance {
                id: string_field(item, "id")
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("reset-{}", index + 1)),
                label: format!("重置机会 {:02}", index + 1),
                expires_at,
            })
        })
        .collect();

    let available = value
        .get("available_count")
        .and_then(number)
        .or_else(|| value.get("availableCount").and_then(number))
        .map(|count| count.max(resets.len() as f64).floor() as u32)
        .unwrap_or(resets.len() as u32);

    (Some(ResetCredits { available }), resets)
}

fn remaining_percent(window: &UsageWindow) -> i64 {
    if window.total <= 0.0 {
        return 0;
    }
    (((window.total - window.used).max(0.0) / window.total) * 100.0).round() as i64
}

fn parse_time(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|date| date.with_timezone(&Utc))
}

fn reset_clock(value: &str) -> String {
    parse_time(value)
        .map(|date| date.with_timezone(&Local).format("%H:%M").to_string())
        .unwrap_or_else(|| "--".to_string())
}

fn reset_days(value: &str) -> String {
    let Some(date) = parse_time(value) else {
        return "--".to_string();
    };
    let seconds = (date - Utc::now()).num_seconds().max(0);
    let days = (seconds + 86_399) / 86_400;
    format!("{days}天")
}

fn reset_duration(value: &str) -> String {
    let Some(date) = parse_time(value) else {
        return "--".to_string();
    };
    let seconds = (date - Utc::now()).num_seconds().max(0);
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3_600;
    let minutes = (seconds % 3_600) / 60;
    if days > 0 {
        format!("{days}天{hours}小时")
    } else if hours > 0 {
        format!("{hours}小时{minutes}分")
    } else {
        format!("{minutes}分")
    }
}

fn build_tray_usage_text(snapshot: &UsageSnapshot) -> Option<TrayUsageText> {
    let five_hour = snapshot.windows.iter().find(|window| window.id == "fiveHour")?;
    let weekly = snapshot.windows.iter().find(|window| window.id == "sevenDay")?;
    let five_hour_percent = remaining_percent(five_hour);
    let weekly_percent = remaining_percent(weekly);
    let title = format!(
        "5小时 {five_hour_percent}% · {}\n每周 {weekly_percent}% · {}",
        reset_clock(&five_hour.reset_at),
        reset_days(&weekly.reset_at)
    );
    let tooltip = format!(
        "CodexInfo · {}\n5小时 {five_hour_percent}% 剩余 · 重置 {}\n每周 {weekly_percent}% 剩余 · {}后重置",
        plan_label(&snapshot.plan_name),
        reset_duration(&five_hour.reset_at),
        reset_days(&weekly.reset_at)
    );
    Some(TrayUsageText {
        title,
        tooltip,
    })
}

fn update_tray_display(app: &AppHandle, state: &AppState) {
    let Some(tray) = app.tray_by_id("main-tray") else {
        return;
    };
    if !state.tray_usage_enabled.load(Ordering::Relaxed) {
        let _ = tray.set_title(None::<&str>);
        let _ = tray.set_tooltip(Some("CodexInfo"));
        return;
    }

    let usage = state
        .latest_tray_usage
        .lock()
        .ok()
        .and_then(|usage| usage.clone());
    if let Some(usage) = usage {
        let _ = tray.set_title(Some(usage.title));
        let _ = tray.set_tooltip(Some(usage.tooltip));
    } else {
        let _ = tray.set_title(None::<&str>);
        let _ = tray.set_tooltip(Some("CodexInfo"));
    }
}

fn update_taskbar_display(app: &AppHandle, state: &AppState) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let _ = state;
    let _ = window.set_skip_taskbar(true);
    let _ = window.set_title("Codex \u{7528}\u{91cf}\u{7ec4}\u{4ef6}");
    let _ = window.set_progress_bar(ProgressBarState {
        status: Some(ProgressBarStatus::None),
        progress: None,
    });
}

#[cfg(target_os = "windows")]
fn tray_notify_rect() -> Option<RECT> {
    unsafe {
        let taskbar = FindWindowW(w!("Shell_TrayWnd"), PCWSTR::null()).ok()?;
        let tray = FindWindowExW(Some(taskbar), None, w!("TrayNotifyWnd"), PCWSTR::null()).ok()?;
        let mut rect = RECT::default();
        GetWindowRect(tray, &mut rect).ok()?;
        if rect.right > rect.left && rect.bottom > rect.top {
            Some(rect)
        } else {
            None
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn tray_notify_rect() -> Option<()> {
    None
}

fn dock_position(app: &AppHandle) -> Option<tauri::PhysicalPosition<f64>> {
    let monitor = app.primary_monitor().ok().flatten()?;
    let monitor_position = monitor.position();
    let monitor_size = monitor.size();
    let work_area = monitor.work_area();
    let scale_factor = monitor.scale_factor();

    let left = monitor_position.x as f64;
    let top = monitor_position.y as f64;
    let right = left + monitor_size.width as f64;
    let bottom = top + monitor_size.height as f64;
    let work_left = work_area.position.x as f64;
    let work_top = work_area.position.y as f64;
    let work_right = work_left + work_area.size.width as f64;
    let work_bottom = work_top + work_area.size.height as f64;
    let width = DOCK_WIDTH * scale_factor;
    let height = DOCK_HEIGHT * scale_factor;
    let tray_gap = DOCK_TRAY_GAP * scale_factor;
    let edge_gap = DOCK_EDGE_GAP * scale_factor;
    let bottom_taskbar = bottom - work_bottom > edge_gap;
    let top_taskbar = work_top - top > edge_gap;

    let x = if bottom_taskbar {
        #[cfg(target_os = "windows")]
        {
            if let Some(rect) = tray_notify_rect() {
                let tray_left = rect.left as f64;
                if tray_left > left && tray_left < right {
                    (tray_left - width - tray_gap).max(left + edge_gap)
                } else {
                    work_right - width - edge_gap
                }
            } else {
                work_right - width - edge_gap
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            work_right - width - edge_gap
        }
    } else if right - work_right > edge_gap {
        work_right - width - edge_gap
    } else {
        right - width - tray_gap
    }
    .max(left + edge_gap)
    .min(right - width - edge_gap);

    let y = if bottom_taskbar {
        let taskbar_height = bottom - work_bottom;
        work_bottom + ((taskbar_height - height) / 2.0).max(0.0)
    } else if top_taskbar {
        let taskbar_height = work_top - top;
        top + ((taskbar_height - height) / 2.0).max(0.0)
    } else {
        bottom - height - edge_gap
    }
    .max(top + edge_gap)
    .min(bottom - height - edge_gap);

    Some(tauri::PhysicalPosition::new(x, y))
}

#[cfg(target_os = "windows")]
fn pin_dock_window(window: &WebviewWindow, position: tauri::PhysicalPosition<f64>) {
    if let Ok(hwnd) = window.hwnd() {
        unsafe {
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                position.x.round() as i32,
                position.y.round() as i32,
                0,
                0,
                SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn pin_dock_window(_: &WebviewWindow, _: tauri::PhysicalPosition<f64>) {}

fn configure_dock_window_style(window: &WebviewWindow) {
    let _ = window.set_focusable(false);
    let _ = window.set_skip_taskbar(true);
    let _ = window.set_shadow(false);
}

fn show_dock_window(window: &WebviewWindow) {
    configure_dock_window_style(window);
    let _ = window.show();
}

#[cfg(target_os = "windows")]
fn keep_dock_window_topmost(window: &WebviewWindow) {
    if let Ok(hwnd) = window.hwnd() {
        unsafe {
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_TOP),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn keep_dock_window_topmost(window: &WebviewWindow) {
    let _ = window.set_always_on_top(true);
}

fn refresh_dock_z_order(app: &AppHandle, state: &AppState) {
    if !state.dock_enabled.load(Ordering::Relaxed) {
        return;
    }
    let Some(window) = app.get_webview_window("dock") else {
        return;
    };

    configure_dock_window_style(&window);
    if !window.is_visible().unwrap_or(false) {
        show_dock_window(&window);
    }
    keep_dock_window_topmost(&window);
}

#[cfg(target_os = "windows")]
fn watched_shell_window(hwnd: HWND) -> bool {
    if hwnd.0.is_null() {
        return false;
    }

    let mut class_name = [0u16; 128];
    let len = unsafe { GetClassNameW(hwnd, &mut class_name) };
    if len <= 0 {
        return false;
    }

    matches!(
        String::from_utf16_lossy(&class_name[..len as usize]).as_str(),
        "Shell_TrayWnd" | "TrayNotifyWnd" | "NotifyIconOverflowWindow" | "TopLevelWindowForOverflowXamlIsland"
    )
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn dock_z_order_event_proc(
    _: HWINEVENTHOOK,
    _: u32,
    hwnd: HWND,
    _: i32,
    _: i32,
    _: u32,
    _: u32,
) {
    if watched_shell_window(hwnd) {
        if let Some(sender) = DOCK_Z_ORDER_EVENTS.get() {
            let _ = sender.send(());
        }
    }
}

#[cfg(target_os = "windows")]
fn start_dock_z_order_watcher(app: AppHandle, state: AppState) {
    let (tx, rx) = mpsc::channel::<()>();
    let _ = DOCK_Z_ORDER_EVENTS.set(tx);

    let pulse_app = app.clone();
    let pulse_state = state.clone();
    thread::spawn(move || {
        while rx.recv().is_ok() {
            for _ in 0..24 {
                refresh_dock_z_order(&pulse_app, &pulse_state);
                thread::sleep(Duration::from_millis(16));
            }
            while rx.try_recv().is_ok() {}
        }
    });

    thread::spawn(move || unsafe {
        let hook = SetWinEventHook(
            EVENT_OBJECT_SHOW,
            EVENT_OBJECT_LOCATIONCHANGE,
            None,
            Some(dock_z_order_event_proc),
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        );
        if hook.0.is_null() {
            return;
        }

        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).as_bool() {}
    });
}

#[cfg(not(target_os = "windows"))]
fn start_dock_z_order_watcher(_: AppHandle, _: AppState) {}

#[cfg(target_os = "windows")]
fn dock_is_clear_of_tray(window: &WebviewWindow, scale_factor: f64) -> bool {
    let Some(tray) = tray_notify_rect() else {
        return false;
    };
    let Ok(position) = window.outer_position() else {
        return false;
    };
    let Ok(size) = window.outer_size() else {
        return false;
    };

    let gap = (DOCK_TRAY_GAP * scale_factor).round() as i32;
    let dock_left = position.x;
    let dock_right = position.x + size.width as i32;
    let dock_top = position.y;
    let dock_bottom = position.y + size.height as i32;
    let vertically_overlaps = dock_bottom > tray.top && dock_top < tray.bottom;
    if !vertically_overlaps {
        return true;
    }

    dock_right + gap <= tray.left || dock_left >= tray.right + gap
}

#[cfg(not(target_os = "windows"))]
fn dock_is_clear_of_tray(_: &WebviewWindow, _: f64) -> bool {
    true
}

fn dock_position_is_stable(state: &AppState, position: tauri::PhysicalPosition<f64>, force: bool) -> bool {
    if force {
        if let Ok(mut pending) = state.dock_pending_position.lock() {
            *pending = Some((position.x.round() as i32, position.y.round() as i32));
        }
        return true;
    }

    let target = (position.x.round() as i32, position.y.round() as i32);
    let Ok(mut pending) = state.dock_pending_position.lock() else {
        return true;
    };
    if pending.as_ref() == Some(&target) {
        true
    } else {
        *pending = Some(target);
        false
    }
}

fn move_dock_window(window: &WebviewWindow, position: tauri::PhysicalPosition<f64>) {
    #[cfg(target_os = "windows")]
    {
        pin_dock_window(window, position);
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = window.set_position(position);
    }
}

fn update_dock_window(app: &AppHandle, state: &AppState, force: bool) {
    let Some(window) = app.get_webview_window("dock") else {
        return;
    };

    let enabled = state.dock_enabled.load(Ordering::Relaxed);
    let visible = window.is_visible().unwrap_or(false);

    if enabled {
        configure_dock_window_style(&window);
        let scale_factor = app
            .primary_monitor()
            .ok()
            .flatten()
            .map(|monitor| monitor.scale_factor())
            .unwrap_or(1.0);
        let should_check_position = force || !visible || !dock_is_clear_of_tray(&window, scale_factor);

        if should_check_position {
            if let Some(position) = dock_position(app) {
                if dock_position_is_stable(state, position, force) {
                    let target_x = position.x.round() as i32;
                    let target_y = position.y.round() as i32;
                    let should_move = window
                        .outer_position()
                        .map(|current| (current.x - target_x).abs() > 1 || (current.y - target_y).abs() > 1)
                        .unwrap_or(true);
                    if should_move {
                        move_dock_window(&window, position);
                    }
                }
            }
        } else if let Ok(mut pending) = state.dock_pending_position.lock() {
            *pending = None;
        }

        if !visible {
            show_dock_window(&window);
        }
        keep_dock_window_topmost(&window);
    } else if visible {
        let _ = window.hide();
    }
}

fn set_latest_tray_usage(app: &AppHandle, snapshot: &UsageSnapshot) {
    let state = app.state::<AppState>();
    if let Ok(mut usage) = state.latest_tray_usage.lock() {
        *usage = build_tray_usage_text(snapshot);
    }
    update_tray_display(app, &state);
    update_taskbar_display(app, &state);
}

fn fetch_reset_credits_from_chatgpt() -> Result<(Option<ResetCredits>, Vec<ResetChance>), String> {
    let (auth_file, mut auth) = load_codex_auth()?;
    let client = Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|error| format!("无法创建 ChatGPT 用量客户端: {error}"))?;

    let response = match request_chatgpt_json(&client, &auth, "wham/rate-limit-reset-credits") {
        Ok(value) => value,
        Err(error) if error == "unauthorized" => {
            refresh_codex_auth(&client, &auth_file, &mut auth)?;
            request_chatgpt_json(&client, &auth, "wham/rate-limit-reset-credits")?
        }
        Err(error) => return Err(error),
    };

    Ok(reset_details_from_chatgpt_response(&response))
}

fn snapshot_from_app_server(app_server: Value, session_token: Option<TokenUsage>) -> Option<DiscoveredSnapshot> {
    let rate_limits = app_server.get("rateLimits")?;
    let bucket = pick_codex_bucket(rate_limits)?;
    let mut windows = Vec::new();

    if let Some(window) = app_server_window_to_usage("fiveHour", "5 小时用量", bucket.get("primary")) {
        windows.push(window);
    }
    if let Some(window) = app_server_window_to_usage("sevenDay", "7 天用量", bucket.get("secondary")) {
        windows.push(window);
    }
    if windows.is_empty() {
        return None;
    }

    let reset_credit_value = rate_limits.get("rateLimitResetCredits");
    let resets = extract_reset_list(reset_credit_value);
    let reset_credits = extract_reset_count(reset_credit_value, resets.len());
    let plan = string_field(bucket, "planType")
        .or_else(|| {
            app_server
                .get("account")
                .and_then(|account_result| account_result.get("account"))
                .and_then(|account| string_field(account, "planType"))
        })
        .map(plan_label)
        .unwrap_or_else(|| "Codex".to_string());
    let subscription_expires_at = app_server
        .get("account")
        .and_then(|account_result| account_result.get("account"))
        .and_then(|account| {
            string_field(account, "subscriptionExpiresAt")
                .or_else(|| string_field(account, "subscriptionExpires"))
        })
        .unwrap_or("")
        .to_string();
    let token_usage = session_token.unwrap_or(TokenUsage {
        input: None,
        cached_input: None,
        output: None,
        reasoning_output: None,
        total: None,
        limit: None,
    });

    let snapshot = UsageSnapshot {
        plan_name: plan,
        subscription_expires_at,
        windows,
        token_usage,
        credits_remaining: 0.0,
        credits_total: 0.0,
        resets,
        reset_credits,
        updated_at: Utc::now().to_rfc3339(),
        source_path: "codex app-server".to_string(),
        account_source: "codexAppServer".to_string(),
        cached_at: None,
        token_periods: Vec::new(),
    };

    Some(DiscoveredSnapshot {
        snapshot,
        source_path: "codex app-server".to_string(),
        account_source: "codexAppServer".to_string(),
    })
}

fn write_usage_cache(app: &AppHandle, snapshot: &UsageSnapshot) -> Result<(), String> {
    let mut cached = serde_json::to_value(snapshot).map_err(|error| error.to_string())?;
    cached["cachedAt"] = Value::String(Utc::now().to_rfc3339());
    let path = usage_cache_path(app)?;
    fs::write(path, serde_json::to_string_pretty(&cached).map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())
}

fn read_explicit_snapshot(app: &AppHandle) -> Option<DiscoveredSnapshot> {
    let mut paths = Vec::new();
    if let Some(path) = std::env::var_os("CODEX_USAGE_FILE") {
        paths.push(PathBuf::from(path));
    }
    if let Ok(app_data) = app.path().app_data_dir() {
        paths.push(app_data.join("codex-usage.json"));
    }
    if let Some(root) = codex_root() {
        paths.push(root.join("codex-usage.json"));
    }

    for path in paths {
        if !path.exists() {
            continue;
        }
        let content = fs::read_to_string(&path).ok()?;
        let mut snapshot: UsageSnapshot = serde_json::from_str(&content).ok()?;
        snapshot.source_path = path.display().to_string();
        snapshot.account_source = "usageFile".to_string();
        return Some(DiscoveredSnapshot {
            source_path: path.display().to_string(),
            account_source: "usageFile".to_string(),
            snapshot,
        });
    }
    None
}

#[tauri::command]
fn get_cached_usage_snapshot(app: AppHandle) -> Result<Option<UsageSnapshot>, String> {
    let path = usage_cache_path(&app)?;
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let snapshot = serde_json::from_str::<UsageSnapshot>(&content).map_err(|error| error.to_string())?;
    set_latest_tray_usage(&app, &snapshot);
    Ok(Some(snapshot))
}

#[tauri::command]
async fn get_usage_snapshot(app: AppHandle) -> Result<UsageSnapshot, String> {
    tauri::async_runtime::spawn_blocking(move || get_usage_snapshot_blocking(app))
        .await
        .map_err(|error| format!("用量读取任务失败: {error}"))?
}

fn get_usage_snapshot_blocking(app: AppHandle) -> Result<UsageSnapshot, String> {
    let explicit = read_explicit_snapshot(&app);
    let session_events = find_latest_token_counts();
    let latest_session = find_latest_token_count(&session_events);
    let session_token = token_usage_from_session(latest_session.as_ref());
    let official = if explicit.is_none() {
        read_codex_app_server().and_then(|value| snapshot_from_app_server(value, session_token))
    } else {
        None
    };
    let session_snapshot = latest_session
        .as_ref()
        .and_then(|event| snapshot_from_token_count(event).ok())
        .map(|snapshot| DiscoveredSnapshot {
            source_path: snapshot.source_path.clone(),
            account_source: "usageFile".to_string(),
            snapshot,
        });

    let discovered = explicit.or(official).or(session_snapshot).ok_or_else(|| {
        "未找到 Codex 用量数据，也无法从 Codex app-server 读取当前账号用量。".to_string()
    })?;

    let mut snapshot = discovered.snapshot;
    snapshot.source_path = discovered.source_path;
    snapshot.account_source = discovered.account_source;
    if snapshot.token_periods.is_empty() {
        snapshot.token_periods = build_token_periods(&session_events);
    }
    if snapshot.account_source == "codexAppServer" {
        let (reset_credits, resets) = fetch_reset_credits_from_chatgpt()
            .map_err(|error| format!("读取重置机会失败: {error}"))?;
        snapshot.reset_credits = reset_credits;
        snapshot.resets = resets;
    }
    write_usage_cache(&app, &snapshot)?;
    set_latest_tray_usage(&app, &snapshot);
    Ok(snapshot)
}

#[tauri::command]
fn get_settings(app: AppHandle) -> Result<AppSettings, String> {
    let launch_at_startup = app
        .autolaunch()
        .is_enabled()
        .map_err(|error| error.to_string())?;
    let stored = read_stored_settings(&app)?;
    let dock_enabled = stored.dock_enabled || stored.taskbar_usage_enabled;
    Ok(AppSettings {
        launch_at_startup,
        main_visible: stored.main_visible,
        keep_always_on_top: stored.keep_always_on_top,
        tray_usage_enabled: stored.tray_usage_enabled,
        dock_enabled,
        taskbar_usage_enabled: dock_enabled,
    })
}

#[tauri::command]
fn set_launch_at_startup(app: AppHandle, enabled: bool) -> Result<AppSettings, String> {
    if enabled {
        app.autolaunch().enable().map_err(|error| error.to_string())?;
    } else {
        app.autolaunch().disable().map_err(|error| error.to_string())?;
    }
    get_settings(app)
}

#[tauri::command]
fn set_tray_usage_enabled(app: AppHandle, enabled: bool) -> Result<AppSettings, String> {
    let mut stored = read_stored_settings(&app)?;
    stored.tray_usage_enabled = enabled;
    write_stored_settings(&app, &stored)?;
    let state = app.state::<AppState>();
    state.tray_usage_enabled.store(enabled, Ordering::Relaxed);
    update_tray_display(&app, &state);
    get_settings(app)
}

#[tauri::command]
fn set_dock_enabled(app: AppHandle, enabled: bool) -> Result<AppSettings, String> {
    let mut stored = read_stored_settings(&app)?;
    stored.dock_enabled = enabled;
    stored.taskbar_usage_enabled = false;
    write_stored_settings(&app, &stored)?;
    let state = app.state::<AppState>();
    state.dock_enabled.store(enabled, Ordering::Relaxed);
    state.taskbar_usage_enabled.store(false, Ordering::Relaxed);
    update_taskbar_display(&app, &state);
    update_dock_window(&app, &state, true);
    get_settings(app)
}

#[tauri::command]
fn set_taskbar_usage_enabled(app: AppHandle, enabled: bool) -> Result<AppSettings, String> {
    let mut stored = read_stored_settings(&app)?;
    stored.dock_enabled = enabled;
    stored.taskbar_usage_enabled = false;
    write_stored_settings(&app, &stored)?;
    let state = app.state::<AppState>();
    state.dock_enabled.store(enabled, Ordering::Relaxed);
    state.taskbar_usage_enabled.store(false, Ordering::Relaxed);
    update_taskbar_display(&app, &state);
    update_dock_window(&app, &state, true);
    get_settings(app)
}

#[tauri::command]
fn set_expanded(app: AppHandle, expanded: bool, extra_height: Option<u32>) -> Result<(), String> {
    let Some(window) = app.get_webview_window("main") else {
        return Ok(());
    };
    let size = if expanded {
        let height = (COLLAPSED_HEIGHT + extra_height.unwrap_or(0) as f64).min(MAX_EXPANDED_HEIGHT);
        tauri::LogicalSize::new(COLLAPSED_WIDTH, height)
    } else {
        tauri::LogicalSize::new(COLLAPSED_WIDTH, COLLAPSED_HEIGHT)
    };
    window.set_size(size).map_err(|error| error.to_string())?;
    let state = app.state::<AppState>();
    if state.hover_component_visible.load(Ordering::Relaxed) {
        position_hover_component(&app, &window);
    }
    Ok(())
}

#[tauri::command]
fn set_hover_region(app: AppHandle, region: String, active: bool) -> Result<(), String> {
    let state = app.state::<AppState>();
    match region.as_str() {
        "main" => state.main_hovered.store(active, Ordering::Relaxed),
        "dock" => state.dock_hovered.store(active, Ordering::Relaxed),
        _ => return Err(format!("unknown hover region: {region}")),
    }

    if active {
        if region == "dock" || state.hover_component_visible.load(Ordering::Relaxed) {
            show_hover_component(&app, &state);
        }
    } else {
        hide_hover_component_if_idle(app.clone(), state.inner().clone());
    }

    Ok(())
}

fn show_window<R: Runtime>(window: &WebviewWindow<R>) {
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
}

fn hide_main_component(app: &AppHandle, state: &AppState) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
    state.hover_component_visible.store(false, Ordering::Relaxed);
    state.main_hovered.store(false, Ordering::Relaxed);
    update_dock_window(app, state, true);
}

fn hover_component_position(app: &AppHandle, window: &WebviewWindow) -> Option<tauri::PhysicalPosition<i32>> {
    let monitor = app.primary_monitor().ok().flatten()?;
    let work_area = monitor.work_area();
    let monitor_position = monitor.position();
    let monitor_size = monitor.size();
    let size = window.outer_size().ok()?;
    let work_left = work_area.position.x;
    let work_top = work_area.position.y;
    let work_right = work_left + work_area.size.width as i32;
    let work_bottom = work_top + work_area.size.height as i32;
    let monitor_top = monitor_position.y;
    let monitor_bottom = monitor_top + monitor_size.height as i32;
    let edge_gap = (DOCK_EDGE_GAP * monitor.scale_factor()).round() as i32;
    let bottom_taskbar = monitor_bottom - work_bottom > edge_gap;
    let top_taskbar = work_top - monitor_top > edge_gap;

    let fallback_x = work_right - size.width as i32 - edge_gap;
    let dock_right = app
        .get_webview_window("dock")
        .and_then(|dock| {
            let position = dock.outer_position().ok()?;
            let size = dock.outer_size().ok()?;
            Some(position.x + size.width as i32)
        })
        .unwrap_or(work_right - edge_gap);
    let x = (dock_right - size.width as i32)
        .max(work_left + edge_gap)
        .min(fallback_x.max(work_left + edge_gap));
    let y = if top_taskbar {
        work_top + edge_gap
    } else if bottom_taskbar {
        work_bottom - size.height as i32
    } else {
        work_bottom - size.height as i32 - edge_gap
    }
    .max(work_top + edge_gap);

    Some(tauri::PhysicalPosition::new(x, y))
}

fn position_hover_component(app: &AppHandle, window: &WebviewWindow) {
    if let Some(position) = hover_component_position(app, window) {
        let _ = window.set_position(position);
    }
}

fn show_hover_component(app: &AppHandle, state: &AppState) {
    if !state.dock_enabled.load(Ordering::Relaxed) {
        return;
    }
    let Some(window) = app.get_webview_window("main") else {
        return;
    };

    state.hover_component_visible.store(true, Ordering::Relaxed);
    let visible = window.is_visible().unwrap_or(false);
    position_hover_component(app, &window);
    if !visible {
        let _ = window.unminimize();
        let _ = window.set_skip_taskbar(true);
        let _ = window.set_always_on_top(true);
        let _ = window.show();
    }
    start_hover_monitor(app.clone(), state.clone());
}

fn cursor_inside_window(app: &AppHandle, label: &str) -> bool {
    let Some(window) = app.get_webview_window(label) else {
        return false;
    };
    if !window.is_visible().unwrap_or(false) {
        return false;
    }
    let Ok(cursor) = app.cursor_position() else {
        return false;
    };
    let Ok(position) = window.outer_position() else {
        return false;
    };
    let Ok(size) = window.outer_size() else {
        return false;
    };

    let left = position.x as f64;
    let top = position.y as f64;
    let right = left + size.width as f64;
    let bottom = top + size.height as f64;
    cursor.x >= left && cursor.x <= right && cursor.y >= top && cursor.y <= bottom
}

fn cursor_inside_hover_area(app: &AppHandle) -> bool {
    cursor_inside_window(app, "main") || cursor_inside_window(app, "dock")
}

fn start_hover_monitor(app: AppHandle, state: AppState) {
    if state.hover_monitor_running.swap(true, Ordering::Relaxed) {
        return;
    }

    thread::spawn(move || {
        loop {
            thread::sleep(Duration::from_millis(120));
            if !state.hover_component_visible.load(Ordering::Relaxed) {
                break;
            }
            if cursor_inside_hover_area(&app) {
                continue;
            }
            hide_main_component(&app, &state);
            break;
        }
        state.hover_monitor_running.store(false, Ordering::Relaxed);
    });
}

fn hide_hover_component_if_idle(app: AppHandle, state: AppState) {
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(180));
        let hover_owned = state.hover_component_visible.load(Ordering::Relaxed);
        if cursor_inside_hover_area(&app) || !hover_owned {
            return;
        }
        hide_main_component(&app, &state);
    });
}

fn show_window_near_tray<R: Runtime>(window: &WebviewWindow<R>, position: tauri::PhysicalPosition<f64>) {
    let scale_factor = window.scale_factor().unwrap_or(1.0);
    let size = window.outer_size().ok();
    let width = size
        .as_ref()
        .map(|size| size.width as f64)
        .unwrap_or(COLLAPSED_WIDTH * scale_factor);
    let height = size
        .as_ref()
        .map(|size| size.height as f64)
        .unwrap_or(COLLAPSED_HEIGHT * scale_factor);
    let x = (position.x - width + 24.0 * scale_factor).max(0.0);
    let y = (position.y - height - 12.0 * scale_factor).max(0.0);
    let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
    show_window(window);
}

fn build_app_menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let stored_settings = read_stored_settings(app).unwrap_or_default();
    let dock_enabled = stored_settings.dock_enabled || stored_settings.taskbar_usage_enabled;
    let visible_item = CheckMenuItem::with_id(
        app,
        "visible",
        "显示组件",
        true,
        stored_settings.main_visible,
        None::<&str>,
    )?;
    let top_item = CheckMenuItem::with_id(
        app,
        "top",
        "保持置顶",
        true,
        stored_settings.keep_always_on_top,
        None::<&str>,
    )?;
    let tray_usage_item = CheckMenuItem::with_id(
        app,
        "tray_usage",
        "托盘显示用量",
        true,
        stored_settings.tray_usage_enabled,
        None::<&str>,
    )?;
    let taskbar_usage_item = CheckMenuItem::with_id(
        app,
        "taskbar_usage",
        "显示任务栏入口",
        true,
        dock_enabled,
        None::<&str>,
    )?;
    let startup_checked = app.autolaunch().is_enabled().unwrap_or(false);
    let startup_item = CheckMenuItem::with_id(app, "startup", "开机启动", true, startup_checked, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    Menu::with_items(
        app,
        &[
            &visible_item,
            &top_item,
            &tray_usage_item,
            &taskbar_usage_item,
            &startup_item,
            &quit_item,
        ],
    )
}

fn handle_menu_event(app: &AppHandle, state: &AppState, id: &str) {
    match id {
        "visible" => {
            if let Some(window) = app.get_webview_window("main") {
                if window.is_visible().unwrap_or(false) {
                    let _ = app.save_window_state(StateFlags::POSITION);
                    hide_main_component(app, state);
                    let mut stored = read_stored_settings(app).unwrap_or_default();
                    stored.main_visible = false;
                    let _ = write_stored_settings(app, &stored);
                } else {
                    show_window(&window);
                    let mut stored = read_stored_settings(app).unwrap_or_default();
                    stored.main_visible = true;
                    let _ = write_stored_settings(app, &stored);
                }
            }
        }
        "top" => {
            let next = !state.keep_always_on_top.load(Ordering::Relaxed);
            state.keep_always_on_top.store(next, Ordering::Relaxed);
            let mut stored = read_stored_settings(app).unwrap_or_default();
            stored.keep_always_on_top = next;
            let _ = write_stored_settings(app, &stored);
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_always_on_top(next);
            }
            if let Some(window) = app.get_webview_window("dock") {
                let _ = window.set_always_on_top(next);
            }
        }
        "tray_usage" => {
            let next = !state.tray_usage_enabled.load(Ordering::Relaxed);
            state.tray_usage_enabled.store(next, Ordering::Relaxed);
            let mut stored = read_stored_settings(app).unwrap_or_default();
            stored.tray_usage_enabled = next;
            let _ = write_stored_settings(app, &stored);
            update_tray_display(app, state);
        }
        "taskbar_usage" => {
            let next = !state.dock_enabled.load(Ordering::Relaxed);
            state.dock_enabled.store(next, Ordering::Relaxed);
            state.taskbar_usage_enabled.store(false, Ordering::Relaxed);
            let mut stored = read_stored_settings(app).unwrap_or_default();
            stored.dock_enabled = next;
            stored.taskbar_usage_enabled = false;
            let _ = write_stored_settings(app, &stored);
            update_taskbar_display(app, state);
            update_dock_window(app, state, true);
        }
        "startup" => {
            let enabled = app.autolaunch().is_enabled().unwrap_or(false);
            if enabled {
                let _ = app.autolaunch().disable();
            } else {
                let _ = app.autolaunch().enable();
            }
        }
        "quit" => {
            let _ = app.save_window_state(StateFlags::POSITION);
            app.exit(0);
        }
        _ => {}
    }
}

#[tauri::command]
fn show_dock_menu(app: AppHandle) -> Result<(), String> {
    let Some(window) = app.get_webview_window("dock") else {
        return Ok(());
    };
    let menu = build_app_menu(&app).map_err(|error| error.to_string())?;
    window.popup_menu(&menu).map_err(|error| error.to_string())
}

fn create_tray(app: &AppHandle, state: AppState) -> tauri::Result<()> {
    let stored_settings = read_stored_settings(app).unwrap_or_default();
    let dock_enabled = stored_settings.dock_enabled || stored_settings.taskbar_usage_enabled;
    state
        .tray_usage_enabled
        .store(stored_settings.tray_usage_enabled, Ordering::Relaxed);
    state
        .keep_always_on_top
        .store(stored_settings.keep_always_on_top, Ordering::Relaxed);
    state
        .dock_enabled
        .store(dock_enabled, Ordering::Relaxed);
    state
        .taskbar_usage_enabled
        .store(false, Ordering::Relaxed);
    let visible_item = CheckMenuItem::with_id(
        app,
        "visible",
        "显示组件",
        true,
        stored_settings.main_visible,
        None::<&str>,
    )?;
    let top_item = CheckMenuItem::with_id(
        app,
        "top",
        "保持置顶",
        true,
        stored_settings.keep_always_on_top,
        None::<&str>,
    )?;
    let tray_usage_item = CheckMenuItem::with_id(
        app,
        "tray_usage",
        "托盘显示用量",
        true,
        stored_settings.tray_usage_enabled,
        None::<&str>,
    )?;
    let taskbar_usage_item = CheckMenuItem::with_id(
        app,
        "taskbar_usage",
        "显示任务栏入口",
        true,
        dock_enabled,
        None::<&str>,
    )?;
    let startup_checked = app.autolaunch().is_enabled().unwrap_or(false);
    let startup_item =
        CheckMenuItem::with_id(app, "startup", "开机启动", true, startup_checked, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &visible_item,
            &top_item,
            &tray_usage_item,
            &taskbar_usage_item,
            &startup_item,
            &quit_item,
        ],
    )?;
    let icon = Image::from_bytes(include_bytes!("../icons/icon.png"))?;
    let tray_state = state.clone();

    TrayIconBuilder::with_id("main-tray")
        .icon(icon)
        .tooltip("CodexInfo")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_tray_icon_event(move |tray, event| {
            if let TrayIconEvent::Click {
                position,
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                if let Some(window) = app.get_webview_window("main") {
                    if window.is_visible().unwrap_or(false) {
                        hide_main_component(app, &tray_state);
                    } else {
                        show_window_near_tray(&window, position);
                        update_dock_window(app, &tray_state, true);
                    }
                }
            }
        })
        .build(app)?;

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let state = AppState {
        keep_always_on_top: Arc::new(AtomicBool::new(true)),
        tray_usage_enabled: Arc::new(AtomicBool::new(true)),
        dock_enabled: Arc::new(AtomicBool::new(true)),
        taskbar_usage_enabled: Arc::new(AtomicBool::new(false)),
        main_hovered: Arc::new(AtomicBool::new(false)),
        dock_hovered: Arc::new(AtomicBool::new(false)),
        hover_component_visible: Arc::new(AtomicBool::new(false)),
        hover_monitor_running: Arc::new(AtomicBool::new(false)),
        dock_pending_position: Arc::new(Mutex::new(None)),
        latest_tray_usage: Arc::new(Mutex::new(None)),
    };
    let menu_state = state.clone();
    let window_state = state.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            None,
        ))
        .manage(state.clone())
        .invoke_handler(tauri::generate_handler![
            get_cached_usage_snapshot,
            get_usage_snapshot,
            get_settings,
            set_launch_at_startup,
            set_tray_usage_enabled,
            set_dock_enabled,
            set_taskbar_usage_enabled,
            set_expanded,
            set_hover_region,
            show_dock_menu
        ])
        .on_menu_event(move |app, event| {
            handle_menu_event(app, &menu_state, event.id.as_ref());
        })
        .setup(move |app| {
            create_tray(app.handle(), state.clone())?;
            let stored_settings = read_stored_settings(app.handle()).unwrap_or_default();
            let dock_enabled = stored_settings.dock_enabled || stored_settings.taskbar_usage_enabled;
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_always_on_top(stored_settings.keep_always_on_top);
                let _ = window.set_skip_taskbar(true);
                let _ = window.set_size(tauri::LogicalSize::new(COLLAPSED_WIDTH, COLLAPSED_HEIGHT));
                if stored_settings.main_visible && !dock_enabled {
                    let _ = window.show();
                    let _ = window.set_focus();
                } else {
                    let _ = window.hide();
                }
            }
            update_dock_window(app.handle(), &state, true);
            let dock_app = app.handle().clone();
            let dock_state = state.clone();
            thread::spawn(move || loop {
                thread::sleep(Duration::from_millis(750));
                update_dock_window(&dock_app, &dock_state, false);
            });
            start_dock_z_order_watcher(app.handle().clone(), state.clone());
            Ok(())
        })
        .on_window_event(move |window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let app = window.app_handle();
                match window.label() {
                    "main" => {
                        let _ = app.save_window_state(StateFlags::POSITION);
                        hide_main_component(app, &window_state);
                    }
                    "dock" => {
                        if window_state.dock_enabled.load(Ordering::Relaxed) {
                            if let Some(dock) = app.get_webview_window("dock") {
                                show_dock_window(&dock);
                                keep_dock_window_topmost(&dock);
                            }
                        } else {
                            let _ = window.hide();
                        }
                    }
                    _ => {
                        let _ = window.hide();
                    }
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
