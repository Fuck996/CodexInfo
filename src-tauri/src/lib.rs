use chrono::{DateTime, Local, Utc};
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
        Arc, Mutex,
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
        Foundation::RECT,
        UI::WindowsAndMessaging::{
            FindWindowExW, FindWindowW, GetWindowRect, SetWindowPos, HWND_TOPMOST,
            SWP_NOACTIVATE, SWP_NOSIZE,
        },
    },
};

const COLLAPSED_WIDTH: f64 = 460.0;
const COLLAPSED_HEIGHT: f64 = 430.0;
const RESET_DETAIL_BASE_HEIGHT: f64 = 130.0;
const RESET_DETAIL_ROW_HEIGHT: f64 = 42.0;
const MAX_EXPANDED_HEIGHT: f64 = 920.0;
const DOCK_WIDTH: f64 = 250.0;
const DOCK_HEIGHT: f64 = 42.0;
const DOCK_TRAY_GAP: f64 = 12.0;
const DOCK_EDGE_GAP: f64 = 4.0;
const APP_SERVER_TIMEOUT_MS: u64 = 8_000;
const MAX_SESSION_FILES_TO_SCAN: usize = 120;
const CHATGPT_BACKEND_URL: &str = "https://chatgpt.com/backend-api";
const OPENAI_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";

#[derive(Clone)]
struct AppState {
    keep_always_on_top: Arc<AtomicBool>,
    tray_usage_enabled: Arc<AtomicBool>,
    dock_enabled: Arc<AtomicBool>,
    taskbar_usage_enabled: Arc<AtomicBool>,
    latest_tray_usage: Arc<Mutex<Option<TrayUsageText>>>,
}

#[derive(Debug, Clone)]
struct TrayUsageText {
    title: String,
    tooltip: String,
    taskbar_title: String,
    taskbar_progress: u64,
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

#[derive(Debug, Serialize, Deserialize)]
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
            main_visible: true,
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

#[derive(Debug)]
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

fn find_latest_token_count() -> Option<TokenCountEvent> {
    let root = sessions_path()?;
    if !root.exists() {
        return None;
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

    let mut latest: Option<TokenCountEvent> = None;
    let mut latest_time = String::new();
    for (path, _) in candidates.into_iter().take(MAX_SESSION_FILES_TO_SCAN) {
        let Some(event) = latest_token_count_in_file(&path) else {
            continue;
        };
        if event.timestamp > latest_time {
            latest_time = event.timestamp.clone();
            latest = Some(event);
        }
    }
    latest
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

        if rate_limits.is_some() {
            let _ = child.kill();
            return Some(json!({
                "account": account,
                "rateLimits": rate_limits,
                "sourcePath": "codex app-server"
            }));
        }
    }

    let _ = child.kill();
    None
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
    let taskbar_title = format!("CodexInfo · 5小时 {five_hour_percent}% · 每周 {weekly_percent}%");
    Some(TrayUsageText {
        title,
        tooltip,
        taskbar_title,
        taskbar_progress: five_hour_percent.clamp(0, 100) as u64,
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
    let enabled = state.taskbar_usage_enabled.load(Ordering::Relaxed);
    let _ = window.set_skip_taskbar(!enabled);
    if !enabled {
        let _ = window.set_title("Codex 用量组件");
        let _ = window.set_progress_bar(ProgressBarState {
            status: Some(ProgressBarStatus::None),
            progress: None,
        });
        return;
    }

    let usage = state
        .latest_tray_usage
        .lock()
        .ok()
        .and_then(|usage| usage.clone());
    if let Some(usage) = usage {
        let _ = window.set_title(&usage.taskbar_title);
        let _ = window.set_progress_bar(ProgressBarState {
            status: Some(ProgressBarStatus::Normal),
            progress: Some(usage.taskbar_progress),
        });
    }
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
        (bottom - height - edge_gap).min(work_bottom + edge_gap)
    } else if top_taskbar {
        (top + edge_gap).max(work_top - height - edge_gap)
    } else {
        bottom - height - edge_gap
    }
    .max(top + edge_gap);

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

fn update_dock_window(app: &AppHandle, state: &AppState) {
    let Some(window) = app.get_webview_window("dock") else {
        return;
    };

    let enabled = state.dock_enabled.load(Ordering::Relaxed);
    let _ = window.set_skip_taskbar(true);
    let _ = window.set_always_on_top(true);
    let _ = window.set_size(tauri::LogicalSize::new(DOCK_WIDTH, DOCK_HEIGHT));

    if enabled {
        if let Some(position) = dock_position(app) {
            let _ = window.set_position(position);
            pin_dock_window(&window, position);
        }
        let _ = window.show();
        let _ = window.set_always_on_top(true);
    } else {
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
    update_dock_window(app, &state);
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
    let explicit = read_explicit_snapshot(&app);
    let latest_session = find_latest_token_count();
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
    Ok(AppSettings {
        launch_at_startup,
        main_visible: stored.main_visible,
        keep_always_on_top: stored.keep_always_on_top,
        tray_usage_enabled: stored.tray_usage_enabled,
        dock_enabled: stored.dock_enabled,
        taskbar_usage_enabled: stored.taskbar_usage_enabled,
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
    write_stored_settings(&app, &stored)?;
    let state = app.state::<AppState>();
    state.dock_enabled.store(enabled, Ordering::Relaxed);
    update_dock_window(&app, &state);
    get_settings(app)
}

#[tauri::command]
fn set_taskbar_usage_enabled(app: AppHandle, enabled: bool) -> Result<AppSettings, String> {
    let mut stored = read_stored_settings(&app)?;
    stored.taskbar_usage_enabled = enabled;
    write_stored_settings(&app, &stored)?;
    let state = app.state::<AppState>();
    state.taskbar_usage_enabled.store(enabled, Ordering::Relaxed);
    update_taskbar_display(&app, &state);
    get_settings(app)
}

#[tauri::command]
fn set_expanded(app: AppHandle, expanded: bool, reset_count: Option<u32>) -> Result<(), String> {
    let Some(window) = app.get_webview_window("main") else {
        return Ok(());
    };
    let size = if expanded {
        let row_count = reset_count.unwrap_or(0).min(12) as f64;
        let height = (COLLAPSED_HEIGHT + RESET_DETAIL_BASE_HEIGHT + RESET_DETAIL_ROW_HEIGHT * row_count)
            .min(MAX_EXPANDED_HEIGHT);
        tauri::LogicalSize::new(COLLAPSED_WIDTH, height)
    } else {
        tauri::LogicalSize::new(COLLAPSED_WIDTH, COLLAPSED_HEIGHT)
    };
    window.set_size(size).map_err(|error| error.to_string())
}

fn show_window<R: Runtime>(window: &WebviewWindow<R>) {
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
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

fn create_tray(app: &AppHandle, state: AppState) -> tauri::Result<()> {
    let stored_settings = read_stored_settings(app).unwrap_or_default();
    state
        .tray_usage_enabled
        .store(stored_settings.tray_usage_enabled, Ordering::Relaxed);
    state
        .keep_always_on_top
        .store(stored_settings.keep_always_on_top, Ordering::Relaxed);
    state
        .dock_enabled
        .store(stored_settings.dock_enabled, Ordering::Relaxed);
    state
        .taskbar_usage_enabled
        .store(stored_settings.taskbar_usage_enabled, Ordering::Relaxed);
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
        stored_settings.taskbar_usage_enabled,
        None::<&str>,
    )?;
    let dock_item = CheckMenuItem::with_id(
        app,
        "dock",
        "任务栏悬浮条",
        true,
        stored_settings.dock_enabled,
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
            &dock_item,
            &tray_usage_item,
            &taskbar_usage_item,
            &startup_item,
            &quit_item,
        ],
    )?;
    let icon = Image::from_bytes(include_bytes!("../icons/icon.png"))?;

    TrayIconBuilder::with_id("main-tray")
        .icon(icon)
        .tooltip("CodexInfo")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                position,
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                if let Some(window) = tray.app_handle().get_webview_window("main") {
                    if window.is_visible().unwrap_or(false) {
                        let _ = window.hide();
                    } else {
                        show_window_near_tray(&window, position);
                    }
                }
            }
        })
        .on_menu_event(move |app, event| match event.id.as_ref() {
            "visible" => {
                if let Some(window) = app.get_webview_window("main") {
                    if window.is_visible().unwrap_or(false) {
                        let _ = app.save_window_state(StateFlags::POSITION);
                        let _ = window.hide();
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
            "dock" => {
                let next = !state.dock_enabled.load(Ordering::Relaxed);
                state.dock_enabled.store(next, Ordering::Relaxed);
                let mut stored = read_stored_settings(app).unwrap_or_default();
                stored.dock_enabled = next;
                let _ = write_stored_settings(app, &stored);
                update_dock_window(app, &state);
            }
            "tray_usage" => {
                let next = !state.tray_usage_enabled.load(Ordering::Relaxed);
                state.tray_usage_enabled.store(next, Ordering::Relaxed);
                let mut stored = read_stored_settings(app).unwrap_or_default();
                stored.tray_usage_enabled = next;
                let _ = write_stored_settings(app, &stored);
                update_tray_display(app, &state);
            }
            "taskbar_usage" => {
                let next = !state.taskbar_usage_enabled.load(Ordering::Relaxed);
                state.taskbar_usage_enabled.store(next, Ordering::Relaxed);
                let mut stored = read_stored_settings(app).unwrap_or_default();
                stored.taskbar_usage_enabled = next;
                let _ = write_stored_settings(app, &stored);
                update_taskbar_display(app, &state);
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
        latest_tray_usage: Arc::new(Mutex::new(None)),
    };

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
            set_expanded
        ])
        .setup(move |app| {
            create_tray(app.handle(), state.clone())?;
            let stored_settings = read_stored_settings(app.handle()).unwrap_or_default();
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_always_on_top(stored_settings.keep_always_on_top);
                let _ = window.set_skip_taskbar(!state.taskbar_usage_enabled.load(Ordering::Relaxed));
                let _ = window.set_size(tauri::LogicalSize::new(COLLAPSED_WIDTH, COLLAPSED_HEIGHT));
                if stored_settings.main_visible {
                    let _ = window.show();
                    let _ = window.set_focus();
                } else {
                    let _ = window.hide();
                }
            }
            update_dock_window(app.handle(), &state);
            let dock_app = app.handle().clone();
            let dock_state = state.clone();
            thread::spawn(move || loop {
                thread::sleep(Duration::from_secs(2));
                update_dock_window(&dock_app, &dock_state);
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.app_handle().save_window_state(StateFlags::POSITION);
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
