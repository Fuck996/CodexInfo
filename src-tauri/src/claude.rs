use super::{empty_token_usage, UsageSnapshot, UsageWindow};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
use std::{
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Reading {
    session_id: String,
    updated_at: DateTime<Utc>,
    rate_limits: Vec<RateLimit>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RateLimit {
    kind: String,
    percent_used: f64,
    resets_at: Option<DateTime<Utc>>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum Export {
    Reading(Reading),
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
        if !limit.percent_used.is_finite() || limit.percent_used < 0.0 {
            return Err("Claude 用量百分比无效".into());
        }
    }
    Ok(())
}

pub fn collect(directory: &Path, input: impl Read) -> Result<(), String> {
    let exported: Export = serde_json::from_reader(input.take(64 * 1024))
        .map_err(|error| format!("Claude 用量数据无效: {error}"))?;
    let (directory, id) = match &exported {
        Export::Reading(reading) => {
            validate(reading)?;
            (directory.to_path_buf(), reading.session_id.clone())
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
    let temporary = directory.join(format!("{id}.{}.tmp", std::process::id()));
    fs::write(
        &temporary,
        serde_json::to_vec(&exported).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    if let Err(error) = fs::rename(&temporary, destination) {
        let _ = fs::remove_file(&temporary);
        return Err(format!("保存 Claude 用量失败: {error}"));
    }
    Ok(())
}

pub fn handle_cli() -> bool {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--claude-usage")) {
        return false;
    }
    let result = args
        .next()
        .ok_or_else(|| "缺少 Claude 用量目录".to_string())
        .and_then(|directory| collect(Path::new(&directory), io::stdin().lock()));
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
    true
}

pub fn read_snapshot(directory: &Path) -> Result<UsageSnapshot, String> {
    if !directory.exists() {
        return Err(
            "尚未收到 Claude 用量。请先接入 Claude Code，再在 Code 会话中发送消息。".into(),
        );
    }
    let mut latest: Option<(Reading, PathBuf)> = None;
    for entry in fs::read_dir(directory).map_err(|error| error.to_string())? {
        let path = entry.map_err(|error| error.to_string())?.path();
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
        if latest
            .as_ref()
            .is_none_or(|(previous, _)| reading.updated_at > previous.updated_at)
        {
            latest = Some((reading, path));
        }
    }
    let (reading, path) = latest.ok_or_else(|| "尚未收到 Claude Code 会话用量".to_string())?;
    let now = Utc::now();
    let windows = reading
        .rate_limits
        .into_iter()
        .filter_map(|limit| {
            let (label, duration) = match limit.kind.as_str() {
                "five_hour" => ("5 小时用量", 300),
                "seven_day" => ("7 天用量", 10_080),
                _ => return None,
            };
            if limit.resets_at.is_some_and(|reset| reset <= now) {
                return None;
            }
            Some(UsageWindow {
                id: limit.kind,
                label: label.into(),
                used: limit.percent_used,
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
        return Err("Claude 尚无有效订阅用量读数，或窗口已到期。请在 Claude Code 会话中继续使用后等待更新。".into());
    }
    Ok(UsageSnapshot {
        cycle_comparisons: Vec::new(),
        provider: super::UsageProvider::Claude,
        plan_name: "Claude 订阅".into(),
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

pub fn token_samples(directory: &Path, cache: &mut std::collections::HashMap<PathBuf, super::CachedSessionUsage>) -> Result<Vec<super::TokenUsageSample>, String> {
    let directory = directory.join("tokens");
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut paths = Vec::new();
    for entry in fs::read_dir(directory).map_err(|error| error.to_string())? {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path.extension().and_then(|extension| extension.to_str()) == Some("json") { paths.push(path); }
    }
    cache.retain(|path, _| paths.contains(path));
    let mut samples = Vec::new();
    for path in paths {
        super::refresh_usage_file(&path, cache, |reader, _| {
        let exported: Export = serde_json::from_reader(reader).map_err(|error| error.to_string())?;
        let Export::Sample {
            timestamp, usage, ..
        } = exported
        else {
            return Err("Claude Token 目录包含无效记录类型".into());
        };
        let input =
            usage.input_tokens + usage.cache_read_input_tokens + usage.cache_creation_input_tokens;
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
        Ok(super::SessionUsage { latest: None, samples: vec![sample] })
        })?;
        samples.extend(cache.get(&path).expect("usage file cached").usage.samples.iter().cloned());
    }
    Ok(samples)
}

pub fn install(root: &Path, executable: &Path) -> Result<String, String> {
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
    let cli = find_cli()?;
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
    Ok("Claude Code 已接入。在已打开的 Code 会话中执行 /reload-plugins，或新建会话；收到模型回复后会更新用量。".into())
}

fn run_cli(cli: &Path, args: &[&str]) -> Result<(), String> {
    let mut command = Command::new(cli);
    command.args(args);
    #[cfg(target_os = "windows")]
    command.creation_flags(0x08000000);
    let mut child = command
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
                    _ => "Claude Code 插件安装超时，已停止安装进程".into(),
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
    if !status?.success() {
        return Err(format!(
            "Claude Code 插件安装失败: {}{}",
            String::from_utf8_lossy(&stdout),
            String::from_utf8_lossy(&stderr)
        ));
    }
    Ok(())
}

fn find_cli() -> Result<PathBuf, String> {
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
                let reading = json!({"type":"reading","sessionId":"test-session","updatedAt":Utc::now(),"rateLimits":[{"kind":"five_hour","percentUsed":used,"resetsAt":(Utc::now()+chrono::Duration::hours(5)).to_rfc3339()}]});
                collect(&directory, reading.to_string().as_bytes())?;
            }
            assert_eq!(read_snapshot(&directory)?.windows[0].used, 25.0);
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
}
