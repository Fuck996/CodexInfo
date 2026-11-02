use super::{empty_token_usage, UsageSnapshot, UsageWindow};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Reading {
    #[serde(default)]
    plan_name: Option<String>,
    #[serde(default)]
    plan_error: Option<String>,
    session_id: String,
    updated_at: DateTime<Utc>,
    rate_limits: Vec<RateLimit>,
    #[serde(default)]
    source: ReadingSource,
}

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum ReadingSource {
    #[default]
    Session,
    Account,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SyncFailure {
    updated_at: DateTime<Utc>,
    message: String,
    retry_after_secs: u32,
    #[serde(default)]
    status_code: Option<u16>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PollLease {
    next_at: DateTime<Utc>,
}

const POLL_INTERVAL_SECS: i64 = 120;
const MAX_READING_AGE_SECS: i64 = 5 * 60;

fn poll_lock(directory: &Path) -> Result<File, String> {
    fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.join(".poll.lock"))
        .map_err(|error| error.to_string())?;
    lock.lock().map_err(|error| error.to_string())?;
    Ok(lock)
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    fs::write(
        &temporary,
        serde_json::to_vec(value).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(format!("保存 Claude 用量失败: {error}"));
    }
    Ok(())
}

fn claim_poll(directory: &Path, now: DateTime<Utc>) -> Result<bool, String> {
    let _lock = poll_lock(directory)?;
    let path = directory.join("poll-lease.json");
    if path.exists() {
        let lease: PollLease =
            serde_json::from_slice(&fs::read(&path).map_err(|error| error.to_string())?)
                .map_err(|error| format!("Claude 查询限频状态损坏: {error}"))?;
        if lease.next_at > now {
            return Ok(false);
        }
    }
    write_json(
        &path,
        &PollLease {
            next_at: now + chrono::Duration::seconds(POLL_INTERVAL_SECS),
        },
    )?;
    Ok(true)
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RateLimit {
    kind: String,
    percent_used: Option<f64>,
    resets_at: Option<DateTime<Utc>>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum Export {
    Reading(Reading),
    SyncFailure(SyncFailure),
    Sample {
        id: String,
        timestamp: String,
        usage: RequestUsage,
    },
}

#[derive(Serialize, Deserialize)]
struct RequestUsage {
    model: String,
    input_tokens: f64,
    output_tokens: f64,
    cache_read_input_tokens: f64,
    cache_creation_input_tokens: f64,
}

fn validate(reading: &Reading) -> Result<(), String> {
    if reading.session_id.is_empty()
        || reading.session_id.len() > 128
        || !reading
            .session_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err("Claude 会话标识无效".into());
    }
    for limit in &reading.rate_limits {
        if limit.percent_used.is_some_and(|used| !used.is_finite() || used < 0.0) {
            return Err("Claude 用量百分比无效".into());
        }
    }
    Ok(())
}

pub fn collect(directory: &Path, input: impl Read) -> Result<(), String> {
    let exported: Export = serde_json::from_reader(input.take(64 * 1024))
        .map_err(|error| format!("Claude 用量数据无效: {error}"))?;
    let (directory, id) = match &exported {
        Export::SyncFailure(failure) => {
            if failure.message.is_empty() || failure.message.len() > 4096 {
                return Err("Claude 同步错误说明无效".into());
            }
            let _lock = poll_lock(directory)?;
            write_json(&directory.join("sync-failure.json"), failure)?;
            let retry = failure.retry_after_secs.max(POLL_INTERVAL_SECS as u32);
            let mut next_at = Utc::now() + chrono::Duration::seconds(i64::from(retry));
            let lease_path = directory.join("poll-lease.json");
            if lease_path.exists() {
                let lease: PollLease = serde_json::from_slice(&fs::read(&lease_path).map_err(|error| error.to_string())?)
                    .map_err(|error| format!("Claude 查询限频状态损坏: {error}"))?;
                next_at = next_at.max(lease.next_at);
            }
            return write_json(
                &lease_path,
                &PollLease { next_at },
            );
        }
        Export::Reading(reading) => {
            validate(reading)?;
            let id = if reading.source == ReadingSource::Account {
                "account".into()
            } else {
                reading.session_id.clone()
            };
            (directory.to_path_buf(), id)
        }
        Export::Sample {
            id,
            timestamp,
            usage,
        } => {
            if id.is_empty()
                || id.len() > 256
                || !id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            {
                return Err("Claude 用量记录标识无效".into());
            }
            DateTime::parse_from_rfc3339(timestamp).map_err(|error| error.to_string())?;
            if [
                usage.input_tokens,
                usage.output_tokens,
                usage.cache_read_input_tokens,
                usage.cache_creation_input_tokens,
            ]
            .iter()
            .any(|value| !value.is_finite() || *value < 0.0)
            {
                return Err("Claude Token 数值无效".into());
            }
            (directory.join("tokens"), id.clone())
        }
    };
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let destination = directory.join(format!("{id}.json"));
    write_json(&destination, &exported)
}

pub fn handle_cli() -> bool {
    let mut args = std::env::args_os().skip(1);
    let command = args.next();
    let claim = command.as_deref() == Some(std::ffi::OsStr::new("--claude-poll-claim"));
    let install_bridge = command.as_deref() == Some(std::ffi::OsStr::new("--claude-install"));
    if !claim && !install_bridge && command.as_deref() != Some(std::ffi::OsStr::new("--claude-usage")) {
        return false;
    }
    let result = args
        .next()
        .ok_or_else(|| "缺少 Claude 用量目录".to_string())
        .and_then(|directory| {
            if install_bridge {
                let executable = std::env::current_exe().map_err(|error| error.to_string())?;
                println!("{}", install(Path::new(&directory), &executable)?);
                Ok(())
            } else if claim {
                let granted = claim_poll(Path::new(&directory), Utc::now())?;
                println!("{}", serde_json::json!({"granted":granted}));
                Ok(())
            } else {
                collect(Path::new(&directory), io::stdin().lock())
            }
        });
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
    true
}

pub fn read_snapshot(directory: &Path) -> Result<UsageSnapshot, String> {
    if !directory.exists() {
        return Err("尚未收到 Claude 账号用量。请接入并重载 Code 插件，保持桥接会话在线。".into());
    }
    let mut latest: Option<(Reading, PathBuf)> = None;
    let failure_path = directory.join("sync-failure.json");
    let failure: Option<SyncFailure> = if failure_path.exists() {
        Some(
            serde_json::from_slice(&fs::read(&failure_path).map_err(|error| error.to_string())?)
                .map_err(|error| format!("Claude 同步状态损坏: {error}"))?,
        )
    } else {
        None
    };
    for entry in fs::read_dir(directory).map_err(|error| error.to_string())? {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path == failure_path
            || path
                .file_name()
                .is_some_and(|name| name == "poll-lease.json")
        {
            continue;
        }
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        let exported: Export =
            serde_json::from_slice(&fs::read(&path).map_err(|error| error.to_string())?)
                .map_err(|error| format!("Claude 用量文件无法解析: {error}"))?;
        let Export::Reading(reading) = exported else {
            return Err("Claude 用量目录包含无效记录类型".into());
        };
        validate(&reading)?;
        if reading.source != ReadingSource::Account {
            continue;
        }
        if latest
            .as_ref()
            .is_none_or(|(previous, _)| reading.updated_at > previous.updated_at)
        {
            latest = Some((reading, path));
        }
    }
    if let Some(failure) = failure {
        if latest
            .as_ref()
            .is_none_or(|(reading, _)| failure.updated_at >= reading.updated_at)
        {
            let failed_at = failure.updated_at.with_timezone(&chrono::Local).format("%m-%d %H:%M:%S");
            if failure.status_code == Some(429) || failure.message == "Claude 账号用量查询失败（HTTP 429）" {
                let lease: PollLease = serde_json::from_slice(&fs::read(directory.join("poll-lease.json")).map_err(|error| error.to_string())?)
                    .map_err(|error| format!("Claude 查询限频状态损坏: {error}"))?;
                let retry = if lease.next_at > Utc::now() {
                    format!("最早 {} 后自动重试", lease.next_at.with_timezone(&chrono::Local).format("%H:%M:%S"))
                } else {
                    "等待下一轮自动重试".into()
                };
                let last_reading = latest.as_ref().map(|(reading, _)|
                    format!("；上次成功 {}", reading.updated_at.with_timezone(&chrono::Local).format("%H:%M:%S")))
                    .unwrap_or_default();
                return Err(format!("Claude 查询被限流（HTTP 429），{retry}{last_reading}。"));
            }
            let status = if Utc::now().signed_duration_since(failure.updated_at).num_seconds() > MAX_READING_AGE_SECS {
                "Claude 账号额度采集已中断"
            } else {
                "Claude 账号用量同步失败"
            };
            return Err(format!("{status}（最后失败：{failed_at}）：{}", failure.message));
        }
    }
    let (reading, path) = latest.ok_or_else(|| {
        "Claude 账号同步尚未启动。请更新接入并在 Code 会话执行 /reload-plugins，保持桥接在线。"
            .to_string()
    })?;
    let now = Utc::now();
    if now.signed_duration_since(reading.updated_at).num_seconds() > MAX_READING_AGE_SECS {
        return Err(format!("Claude 账号用量超过 5 分钟未更新（最近读数：{}）。后台额度采集尚未恢复，当前不能比较。", reading.updated_at.with_timezone(&chrono::Local).format("%m-%d %H:%M:%S")));
    }
    let mut unavailable_windows = Vec::new();
    let windows = reading
        .rate_limits
        .into_iter()
        .filter_map(|limit| {
            let (label, duration) = match limit.kind.as_str() {
                "five_hour" => ("5 小时用量", 300),
                "seven_day" => ("7 天用量", 10_080),
                _ => return None,
            };
            let Some(used) = limit.percent_used else {
                unavailable_windows.push(label);
                return None;
            };
            if limit.resets_at.is_some_and(|reset| reset <= now) {
                return None;
            }
            Some(UsageWindow {
                id: limit.kind,
                label: label.into(),
                used,
                total: 100.0,
                reset_at: limit
                    .resets_at
                    .map(|reset| reset.to_rfc3339())
                    .unwrap_or_default(),
                window_duration_mins: duration,
                display_mode: Some("percent".into()),
            })
        })
        .collect::<Vec<_>>();
    if windows.is_empty() {
        if !unavailable_windows.is_empty() {
            return Err(format!("Claude 暂未提供{}的百分比读数，等待下一次查询。", unavailable_windows.join("、")));
        }
        return Err("Claude 账号暂无有效额度窗口，等待下一次查询；不会把过期读数补为零。".into());
    }
    Ok(UsageSnapshot {
        cycle_comparisons: Vec::new(),
        provider: super::UsageProvider::Claude,
        plan_name: reading.plan_name.unwrap_or_else(|| "套餐未提供".into()),
        plan_error: reading.plan_error,
        usage_warning: (!unavailable_windows.is_empty()).then(|| format!("{}：Claude 暂未提供百分比读数", unavailable_windows.join("、"))),
        subscription_expires_at: String::new(),
        windows,
        token_usage: empty_token_usage(),
        credits_remaining: 0.0,
        credits_total: 0.0,
        resets: Vec::new(),
        reset_credits: None,
        updated_at: reading.updated_at.to_rfc3339(),
        source_path: path.display().to_string(),
        account_source: "claudeMod".into(),
        cached_at: None,
    })
}

pub(super) fn authorization_rejected_at(directory: &Path) -> Result<Option<DateTime<Utc>>, String> {
    let path = directory.join("sync-failure.json");
    if !path.exists() { return Ok(None); }
    let failure: SyncFailure = serde_json::from_slice(&fs::read(path).map_err(|error| error.to_string())?)
        .map_err(|error| format!("Claude 同步状态损坏: {error}"))?;
    Ok((failure.status_code == Some(401)).then_some(failure.updated_at))
}

pub fn token_samples(
    directory: &Path,
    cache: &mut std::collections::HashMap<PathBuf, super::CachedSessionUsage>,
) -> Result<Vec<super::TokenUsageSample>, String> {
    let directory = directory.join("tokens");
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut paths = Vec::new();
    for entry in fs::read_dir(directory).map_err(|error| error.to_string())? {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path.extension().and_then(|extension| extension.to_str()) == Some("json") {
            paths.push(path);
        }
    }
    cache.retain(|path, _| paths.contains(path));
    let mut samples = Vec::new();
    for path in paths {
        super::refresh_usage_file(&path, cache, |reader, _| {
            let exported: Export =
                serde_json::from_reader(reader).map_err(|error| error.to_string())?;
            let Export::Sample {
                timestamp, usage, ..
            } = exported
            else {
                return Err("Claude Token 目录包含无效记录类型".into());
            };
            let input = usage.input_tokens
                + usage.cache_read_input_tokens
                + usage.cache_creation_input_tokens;
            let sample = super::TokenUsageSample {
                timestamp,
                model: usage.model,
                cache_write: usage.cache_creation_input_tokens,
                windows: Vec::new(),
                token_usage: super::TokenUsage {
                    input: Some(input),
                    cached_input: Some(usage.cache_read_input_tokens),
                    output: Some(usage.output_tokens),
                    total: Some(input + usage.output_tokens),
                    reasoning_output: None,
                    limit: None,
                },
            };
            Ok(super::SessionUsage {
                latest: None,
                samples: vec![sample],
            })
        })?;
        samples.extend(
            cache
                .get(&path)
                .expect("usage file cached")
                .usage
                .samples
                .iter()
                .cloned(),
        );
    }
    Ok(samples)
}

pub fn install(root: &Path, executable: &Path) -> Result<String, String> {
    let plugin = prepare_plugin(root, executable)?;
    let marketplace = root.join("claude-bridge");
    let cli = find_cli()?;
    run_cli(&cli, &["plugin", "validate", &plugin.to_string_lossy()])?;
    run_cli(
        &cli,
        &[
            "plugin",
            "marketplace",
            "add",
            &marketplace.to_string_lossy(),
        ],
    )?;
    run_cli(
        &cli,
        &[
            "plugin",
            "install",
            "codexinfo-usage@codexinfo-local",
            "--scope",
            "user",
        ],
    )?;
    run_cli(
        &cli,
        &["plugin", "update", "codexinfo-usage@codexinfo-local"],
    )?;
    Ok("Claude 账号用量已接入。组件会自动启动后台查询宿主；请先完成 Claude Code 官方登录，之后使用 Projects 或普通聊天也能同步账号额度。已有 Code 会话可执行 /reload-plugins 继续采集该会话的 Token。".into())
}

pub(super) fn prepare_plugin(root: &Path, executable: &Path) -> Result<PathBuf, String> {
    let marketplace = root.join("claude-bridge");
    let plugin = marketplace.join("claude-plugin");
    fs::create_dir_all(plugin.join(".claude-plugin")).map_err(|error| error.to_string())?;
    fs::create_dir_all(plugin.join("hooks")).map_err(|error| error.to_string())?;
    fs::create_dir_all(marketplace.join(".claude-plugin")).map_err(|error| error.to_string())?;
    let files = [
        (
            ".claude-plugin/plugin.json",
            include_str!("../../claude-plugin/.claude-plugin/plugin.json"),
        ),
        (
            "hooks/hooks.json",
            include_str!("../../claude-plugin/hooks/hooks.json"),
        ),
        (
            "hooks/register.js",
            include_str!("../../claude-plugin/hooks/register.js"),
        ),
    ];
    for (name, content) in files {
        fs::write(plugin.join(name), content).map_err(|error| error.to_string())?;
    }
    let config = json!({ "executable": executable, "directory": root.join("claude-readings") });
    fs::write(plugin.join("bridge.json"), config.to_string()).map_err(|error| error.to_string())?;
    let manifest = json!({
        "name": "codexinfo-local", "owner": {"name": "CodexInfo"},
        "plugins": [{"name": "codexinfo-usage", "source": "./claude-plugin", "description": "本机用量组件"}]
    });
    fs::write(
        marketplace.join(".claude-plugin/marketplace.json"),
        manifest.to_string(),
    )
    .map_err(|error| error.to_string())?;
    Ok(plugin)
}

fn run_cli(cli: &Path, args: &[&str]) -> Result<(), String> {
    let output = cli_output(cli, args)?;
    if !output.status.success() {
        return Err(format!(
            "Claude Code 插件安装失败: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

pub(super) fn cli_output(cli: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    let mut command = Command::new(cli);
    command.args(args);
    command_output(command)
}

pub(super) fn command_output(mut command: Command) -> Result<std::process::Output, String> {
    #[cfg(target_os = "windows")]
    command.creation_flags(0x08000000);
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("启动 Claude Code 失败: {error}"))?;
    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");
    let out_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(128 * 1024)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let err_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr
            .take(128 * 1024)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if started.elapsed() < Duration::from_secs(60) => {
                thread::sleep(Duration::from_millis(100))
            }
            pending => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(match pending {
                    Err(error) => error.to_string(),
                    _ => "Claude Code 命令超时，已停止子进程".into(),
                });
            }
        }
    };
    let stdout = out_reader
        .join()
        .map_err(|_| "读取安装输出的线程失败".to_string())?
        .map_err(|error| error.to_string())?;
    let stderr = err_reader
        .join()
        .map_err(|_| "读取安装错误的线程失败".to_string())?
        .map_err(|error| error.to_string())?;
    Ok(std::process::Output { status: status?, stdout, stderr })
}

pub(super) fn find_cli() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("PATH")
        .into_iter()
        .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .map(|directory| directory.join("claude.exe"))
        .find(|path| path.is_file())
    {
        return Ok(path);
    }
    let roaming = dirs::config_dir().ok_or_else(|| "无法确定用户配置目录".to_string())?;
    let mut roots = vec![roaming.join("Claude/claude-code")];
    if let Some(local) = dirs::data_local_dir() {
        roots.push(local.join("Claude-Data/claude_pzs8sxrjxfjjc/claude-code"));
        roots.push(
            local.join("Packages/Claude_pzs8sxrjxfjjc/LocalCache/Roaming/Claude/claude-code"),
        );
    }
    let mut binaries = Vec::new();
    for root in roots.into_iter().filter(|root| root.exists()) {
        for entry in walkdir::WalkDir::new(root).max_depth(3) {
            let entry = entry.map_err(|error| error.to_string())?;
            if entry.file_name() == "claude.exe" && entry.file_type().is_file() {
                let modified = entry
                    .metadata()
                    .map_err(|error| error.to_string())?
                    .modified()
                    .map_err(|error| error.to_string())?;
                binaries.push((modified, entry.into_path()));
            }
        }
    }
    binaries
        .into_iter()
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, path)| path)
        .ok_or_else(|| {
            "未找到 Claude Code。请先安装 Claude 桌面版并打开 Code 页，或安装终端版 Claude Code。"
                .into()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collector_replaces_readings_and_deduplicates_requests() {
        let directory =
            std::env::temp_dir().join(format!("codexinfo-claude-test-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let result = (|| -> Result<(), String> {
            for used in [10, 25] {
                let reading = json!({"type":"reading","source":"account","sessionId":"test-session","planName":"Claude Pro","updatedAt":Utc::now(),"rateLimits":[{"kind":"five_hour","percentUsed":used,"resetsAt":(Utc::now()+chrono::Duration::hours(5)).to_rfc3339()}]});
                collect(&directory, reading.to_string().as_bytes())?;
            }
            assert_eq!(read_snapshot(&directory)?.windows[0].used, 25.0);
            assert_eq!(read_snapshot(&directory)?.plan_name, "Claude Pro");
            let sample = json!({"type":"sample","id":"test-request-1","timestamp":Utc::now(),"usage":{"model":"claude-opus-5-5","input_tokens":100,"output_tokens":20,"cache_read_input_tokens":200,"cache_creation_input_tokens":50}});
            for _ in 0..2 {
                collect(&directory, sample.to_string().as_bytes())?;
            }
            let samples = token_samples(&directory, &mut std::collections::HashMap::new())?;
            assert_eq!(samples.len(), 1);
            assert_eq!(samples[0].token_usage.total, Some(370.0));
            assert_eq!(samples[0].token_usage.input, Some(350.0));
            Ok(())
        })();
        fs::remove_dir_all(&directory).unwrap();
        result.unwrap();
    }

    #[test]
    fn collector_refuses_path_traversal() {
        let reading = json!({"type":"reading","sessionId":"../escape","updatedAt":Utc::now(),"rateLimits":[]});
        assert!(collect(Path::new("unused"), reading.to_string().as_bytes()).is_err());
    }

    fn test_directory(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "codexinfo-{label}-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        fs::create_dir(&path).unwrap();
        path
    }

    fn account_reading(directory: &Path, timestamp: DateTime<Utc>) {
        let reading = json!({"type":"reading","source":"account","sessionId":"account","updatedAt":timestamp,
            "rateLimits":[{"kind":"five_hour","percentUsed":42.5,"resetsAt":(Utc::now()+chrono::Duration::hours(5)).to_rfc3339()}]});
        collect(directory, reading.to_string().as_bytes()).unwrap();
    }

    #[test]
    fn nullable_reset_preserves_real_usage_and_weekly_analytics() {
        let directory = test_directory("nullable-reset");
        let reset = (Utc::now() + chrono::Duration::days(5)).to_rfc3339();
        let reading = json!({"type":"reading","source":"account","sessionId":"account","updatedAt":Utc::now(),
            "rateLimits":[{"kind":"five_hour","percentUsed":12.5,"resetsAt":null},
                {"kind":"seven_day","percentUsed":70,"resetsAt":reset}]});
        collect(&directory, reading.to_string().as_bytes()).unwrap();
        let mut snapshot = read_snapshot(&directory).unwrap();
        assert_eq!(snapshot.windows.len(), 2);
        assert_eq!(snapshot.windows[0].used, 12.5);
        assert!(snapshot.windows[0].reset_at.is_empty());
        assert_eq!(snapshot.windows[1].used, 70.0);
        assert!(snapshot.usage_warning.is_none());
        crate::analytics::enrich(&directory, &mut crate::analytics::History::default(), &mut snapshot, &[]).unwrap();
        assert_eq!(snapshot.cycle_comparisons.len(), 1);
        assert_eq!(serde_json::to_value(&snapshot.cycle_comparisons).unwrap()[0]["windowId"], "seven_day");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn unknown_window_usage_is_explicit_and_does_not_discard_weekly_readings() {
        let directory = test_directory("nullable-utilization");
        let reading = json!({"type":"reading","source":"account","sessionId":"account","updatedAt":Utc::now(),
            "rateLimits":[{"kind":"five_hour","percentUsed":null,"resetsAt":null},
                {"kind":"seven_day","percentUsed":70,"resetsAt":(Utc::now()+chrono::Duration::days(5)).to_rfc3339()}]});
        collect(&directory, reading.to_string().as_bytes()).unwrap();
        let snapshot = read_snapshot(&directory).unwrap();
        assert_eq!(snapshot.windows.len(), 1);
        assert_eq!(snapshot.windows[0].id, "seven_day");
        assert_eq!(snapshot.windows[0].used, 70.0);
        assert!(snapshot.usage_warning.unwrap().contains("5 小时用量"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn entirely_unknown_usage_never_becomes_zero_usage() {
        let directory = test_directory("all-nullable-utilization");
        let reading = json!({"type":"reading","source":"account","sessionId":"account","updatedAt":Utc::now(),
            "rateLimits":[{"kind":"five_hour","percentUsed":null,"resetsAt":null},
                {"kind":"seven_day","percentUsed":null,"resetsAt":null}]});
        collect(&directory, reading.to_string().as_bytes()).unwrap();
        assert!(read_snapshot(&directory).unwrap_err().contains("暂未提供5 小时用量、7 天用量的百分比读数"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn account_reading_remains_authoritative_over_newer_session_events() {
        let directory = test_directory("account-source");
        account_reading(&directory, Utc::now() - chrono::Duration::seconds(10));
        let reading = json!({"type":"reading","sessionId":"legacy","updatedAt":Utc::now(),
            "rateLimits":[{"kind":"five_hour","percentUsed":99}]});
        collect(&directory, reading.to_string().as_bytes()).unwrap();
        assert_eq!(read_snapshot(&directory).unwrap().windows[0].used, 42.5);
        assert_eq!(read_snapshot(&directory).unwrap().plan_name, "套餐未提供");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn failed_or_stale_account_reads_are_never_reported_as_connected() {
        let directory = test_directory("account-failure");
        account_reading(&directory, Utc::now() - chrono::Duration::minutes(6));
        assert!(read_snapshot(&directory)
            .unwrap_err()
            .contains("超过 5 分钟"));
        let failure = json!({"type":"syncFailure","updatedAt":Utc::now(),"message":"HTTP 429","retryAfterSecs":700});
        collect(&directory, failure.to_string().as_bytes()).unwrap();
        assert!(read_snapshot(&directory).unwrap_err().contains("HTTP 429"));
        account_reading(&directory, Utc::now() + chrono::Duration::seconds(1));
        assert_eq!(read_snapshot(&directory).unwrap().windows[0].used, 42.5);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn old_network_failures_are_dated_as_interrupted_collection() {
        let directory = test_directory("old-network-failure");
        account_reading(&directory, Utc::now() - chrono::Duration::days(4));
        let failure = json!({"type":"syncFailure","updatedAt":Utc::now() - chrono::Duration::days(3),
            "message":"ECONNREFUSED", "retryAfterSecs":120});
        collect(&directory, failure.to_string().as_bytes()).unwrap();
        let error = read_snapshot(&directory).unwrap_err();
        assert!(error.contains("采集已中断"));
        assert!(error.contains("最后失败："));
        assert!(error.contains("ECONNREFUSED"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn concurrent_sessions_share_poll_budget_and_server_backoff() {
        let directory = test_directory("poll-budget");
        let now = Utc::now();
        let workers: Vec<_> = (0..8)
            .map(|_| {
                let directory = directory.clone();
                thread::spawn(move || claim_poll(&directory, now).unwrap())
            })
            .collect();
        let granted = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .filter(|granted| *granted)
            .count();
        assert_eq!(granted, 1);
        assert!(claim_poll(&directory, now + chrono::Duration::seconds(121)).unwrap());
        let failure =
            json!({"type":"syncFailure","updatedAt":now,"message":"HTTP 429","retryAfterSecs":700});
        collect(&directory, failure.to_string().as_bytes()).unwrap();
        assert!(!claim_poll(&directory, now + chrono::Duration::seconds(300)).unwrap());
        assert!(claim_poll(&directory, now + chrono::Duration::seconds(701)).unwrap());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_later_failure_cannot_shorten_server_cooldown_and_rate_limit_shows_retry_time() {
        let directory = test_directory("server-cooldown");
        let now = Utc::now();
        account_reading(&directory, now - chrono::Duration::minutes(30));
        collect(&directory, json!({"type":"syncFailure","updatedAt":now,"message":"Claude 账号用量查询失败（HTTP 429）",
            "statusCode":429,"retryAfterSecs":3600}).to_string().as_bytes()).unwrap();
        let first: PollLease = serde_json::from_slice(&fs::read(directory.join("poll-lease.json")).unwrap()).unwrap();
        assert!(read_snapshot(&directory).unwrap_err().contains("最早"));
        collect(&directory, json!({"type":"syncFailure","updatedAt":now,"message":"HTTP 401",
            "statusCode":401,"retryAfterSecs":120}).to_string().as_bytes()).unwrap();
        let second: PollLease = serde_json::from_slice(&fs::read(directory.join("poll-lease.json")).unwrap()).unwrap();
        assert_eq!(first.next_at, second.next_at);
        assert_eq!(authorization_rejected_at(&directory).unwrap(), Some(now));
        fs::remove_dir_all(directory).unwrap();
    }
}
