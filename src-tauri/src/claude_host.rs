use super::{claude, diagnostics};
use std::{
    fs,
    io::{self, Read},
    path::Path,
    process::{Child, Command, Stdio},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
#[cfg(target_os = "windows")]
use std::os::windows::{io::{AsRawHandle, FromRawHandle, OwnedHandle}, process::CommandExt};
#[cfg(target_os = "windows")]
use windows::Win32::{
    Foundation::HANDLE,
    System::JobObjects::{AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE},
};

#[derive(Default)]
pub(super) struct Host {
    process: Option<HostProcess>,
    retry: Option<(Instant, String)>,
    renewed_after: Option<chrono::DateTime<chrono::Utc>>,
}

impl Host {
    pub(super) fn ensure_running(&mut self, root: &Path) -> Result<(), String> {
        if let Some(rejected_at) = claude::authorization_rejected_at(&root.join("claude-readings"))? {
            if self.renewed_after.is_none_or(|previous| rejected_at > previous) {
                self.renewed_after = Some(rejected_at);
                diagnostics::record(&format!("claude_host authorization_rejected at={rejected_at}; renewing through official initialization"));
                self.shutdown()?;
                self.retry = None;
            }
        }
        if let Some(process) = &mut self.process {
            if let Some(status) = process.child.try_wait().map_err(|error| error.to_string())? {
                let output = self.process.take().expect("host process present").stop()?;
                let message = format!("Claude 后台额度宿主已退出（{status}）：{output}");
                self.retry = Some((Instant::now(), message.clone()));
                return Err(message);
            }
            return Ok(());
        }
        if let Some((at, message)) = &self.retry {
            if at.elapsed() < Duration::from_secs(120) { return Err(message.clone()); }
        }
        let result = self.start(root);
        if let Err(message) = &result {
            self.retry = Some((Instant::now(), message.clone()));
        }
        result
    }

    fn start(&mut self, root: &Path) -> Result<(), String> {
        if !root.join("claude-bridge/claude-plugin/bridge.json").is_file() {
            return Err("尚未接入 Claude 账号用量，请在显示设置中接入 Claude。".into());
        }
        let executable = std::env::current_exe().map_err(|error| error.to_string())?;
        let plugin = claude::prepare_plugin(root, &executable)?;
        let cli = claude::find_cli()?;
        // Mods 授权只读取现有凭据；官方初始化负责续期，之后由新宿主读取新授权。
        let mut initialization = Command::new(&cli);
        initialization.current_dir(root.join("claude-bridge"))
            .args(["--init-only", "--setting-sources", "", "--strict-mcp-config", "--mcp-config", "{\"mcpServers\":{}}"]);
        apply_network_settings(&mut initialization)?;
        let initialized = claude::command_output(initialization)?;
        if !initialized.status.success() {
            return Err(format!("Claude 官方授权初始化失败：{}", String::from_utf8_lossy(&initialized.stderr).trim()));
        }
        let output = claude::cli_output(&cli, &["auth", "status"])?;
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct AuthStatus { logged_in: bool, auth_method: String }
        let status: AuthStatus = serde_json::from_slice(&output.stdout)
            .map_err(|error| format!("无法解析 Claude 官方登录状态：{error}"))?;
        if !status.logged_in || !matches!(status.auth_method.as_str(), "claude.ai" | "oauth_token") {
            return Err("Claude 后台额度采集尚未登录订阅账号，请完成 Claude Code 官方登录；Desktop 与 CLI 的登录独立。".into());
        }
        if !output.status.success() { return Err("Claude 官方登录状态检查失败".into()); }
        let mut command = Command::new(&cli);
        command.current_dir(root.join("claude-bridge"));
        // 保持输入管道打开但不写入消息，官方宿主只运行额度插件，不启动模型回合。
        command.args(["--print", "--input-format", "stream-json", "--output-format", "stream-json",
            "--verbose", "--no-session-persistence", "--setting-sources", "", "--strict-mcp-config",
            "--mcp-config", "{\"mcpServers\":{}}", "--plugin-dir"]);
        command.arg(plugin).env_remove("CLAUDE_CODE_PLUGIN_DIRS");
        apply_network_settings(&mut command)?;
        let process = HostProcess::spawn(command)?;
        diagnostics::record(&format!("claude_host started pid={} cli={}", process.child.id(), cli.display()));
        self.process = Some(process);
        self.retry = None;
        Ok(())
    }

    pub(super) fn shutdown(&mut self) -> Result<(), String> {
        if let Some(process) = self.process.take() {
            let pid = process.child.id();
            process.stop()?;
            diagnostics::record(&format!("claude_host stopped pid={pid}"));
        }
        Ok(())
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        if let Err(error) = self.shutdown() {
            diagnostics::record(&format!("claude_host shutdown_failed error={error}"));
        }
    }
}

fn apply_network_settings(command: &mut Command) -> Result<(), String> {
    let config = std::env::var_os("CLAUDE_CONFIG_DIR").map(std::path::PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".claude")))
        .ok_or_else(|| "无法确定 Claude 配置目录".to_string())?;
    let path = config.join("settings.json");
    if !path.exists() { return Ok(()); }
    let settings: serde_json::Value = serde_json::from_slice(&fs::read(path).map_err(|error| error.to_string())?)
        .map_err(|error| format!("Claude 网络配置无法解析：{error}"))?;
    if let Some(env) = settings.get("env").and_then(|value| value.as_object()) {
        for key in ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy", "NO_PROXY", "no_proxy",
            "NODE_EXTRA_CA_CERTS", "SSL_CERT_FILE", "SSL_CERT_DIR", "CLAUDE_CODE_CLIENT_CERT",
            "CLAUDE_CODE_CLIENT_KEY", "CLAUDE_CODE_CLIENT_KEY_PASSPHRASE"] {
            if let Some(value) = env.get(key) {
                let value = value.as_str().ok_or_else(|| format!("Claude 网络配置 {key} 必须为字符串"))?;
                command.env(key, value);
            }
        }
    }
    Ok(())
}

struct HostProcess {
    child: Child,
    output: Vec<JoinHandle<io::Result<Vec<u8>>>>,
    #[cfg(target_os = "windows")]
    job: Option<OwnedHandle>,
}

impl HostProcess {
    fn spawn(mut command: Command) -> Result<Self, String> {
        #[cfg(target_os = "windows")]
        command.creation_flags(0x08000000);
        let child = command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
            .spawn().map_err(|error| format!("启动 Claude 后台额度宿主失败：{error}"))?;
        let mut process = Self { child, output: Vec::new(), #[cfg(target_os = "windows")] job: None };
        #[cfg(target_os = "windows")]
        { process.job = Some(own_process_tree(&process.child)?); }
        let stdout = process.child.stdout.take().expect("stdout piped");
        let stderr = process.child.stderr.take().expect("stderr piped");
        process.output.push(drain(stdout));
        process.output.push(drain(stderr));
        Ok(process)
    }

    fn stop(mut self) -> Result<String, String> {
        self.child.stdin.take();
        let started = Instant::now();
        while self.child.try_wait().map_err(|error| error.to_string())?.is_none()
            && started.elapsed() < Duration::from_secs(3) {
            thread::sleep(Duration::from_millis(25));
        }
        #[cfg(target_os = "windows")]
        self.job.take();
        if self.child.try_wait().map_err(|error| error.to_string())?.is_none() {
            self.child.kill().map_err(|error| error.to_string())?;
        }
        self.child.wait().map_err(|error| error.to_string())?;
        let mut output = String::new();
        for reader in self.output.drain(..) {
            let bytes = reader.join().map_err(|_| "Claude 宿主输出线程异常退出".to_string())?
                .map_err(|error| error.to_string())?;
            output.push_str(&String::from_utf8_lossy(&bytes));
        }
        Ok(output.trim().to_string())
    }
}

impl Drop for HostProcess {
    fn drop(&mut self) {
        #[cfg(target_os = "windows")]
        self.job.take();
        if matches!(self.child.try_wait(), Ok(None)) {
            if let Err(error) = self.child.kill().and_then(|_| self.child.wait()) {
                diagnostics::record(&format!("claude_host cleanup_failed error={error}"));
            }
        }
    }
}

fn drain(mut reader: impl Read + Send + 'static) -> JoinHandle<io::Result<Vec<u8>>> {
    thread::spawn(move || {
        let mut tail = Vec::new();
        let mut buffer = [0u8; 4096];
        loop {
            let count = reader.read(&mut buffer)?;
            if count == 0 { return Ok(tail); }
            tail.extend_from_slice(&buffer[..count]);
            if tail.len() > 8192 { tail.drain(..tail.len() - 8192); }
        }
    })
}

#[cfg(target_os = "windows")]
fn own_process_tree(child: &Child) -> Result<OwnedHandle, String> {
    let raw = unsafe { CreateJobObjectW(None, None) }.map_err(|error| error.to_string())?;
    let job = unsafe { OwnedHandle::from_raw_handle(raw.0) };
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    unsafe {
        SetInformationJobObject(raw, JobObjectExtendedLimitInformation,
            (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            std::mem::size_of_val(&limits) as u32)
            .and_then(|_| AssignProcessToJobObject(raw, HANDLE(child.as_raw_handle())))
    }.map_err(|error| format!("关联 Claude 后台进程生命周期失败：{error}"))?;
    Ok(job)
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;
    use windows::Win32::{Foundation::WAIT_OBJECT_0,
        System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE}};

    #[test]
    fn host_sends_no_messages_and_shutdown_closes_stdin() {
        let mut command = Command::new("powershell.exe");
        command.args(["-NoLogo", "-NoProfile", "-Command",
            "[Console]::Out.Write([Console]::In.ReadToEnd().Length)"]);
        let mut process = HostProcess::spawn(command).unwrap();
        assert!(process.child.try_wait().unwrap().is_none());
        assert_eq!(process.stop().unwrap(), "0");
    }

    #[test]
    fn one_authorization_rejection_stops_the_owned_host_once_and_keeps_retry_budget() {
        let root = std::env::temp_dir().join(format!("codexinfo-host-auth-{}", std::process::id()));
        let readings = root.join("claude-readings");
        fs::create_dir_all(&readings).unwrap();
        let rejected_at = chrono::Utc::now();
        claude::collect(&readings, serde_json::json!({"type":"syncFailure","updatedAt":rejected_at,
            "message":"HTTP 401","statusCode":401,"retryAfterSecs":3600}).to_string().as_bytes()).unwrap();
        let lease = fs::read(readings.join("poll-lease.json")).unwrap();
        let mut command = Command::new("powershell.exe");
        command.args(["-NoLogo", "-NoProfile", "-Command", "[Console]::In.ReadToEnd()"]);
        let mut host = Host { process: Some(HostProcess::spawn(command).unwrap()), retry: None, renewed_after: None };
        assert!(host.ensure_running(&root).is_err());
        assert!(host.process.is_none());
        assert_eq!(host.renewed_after, Some(rejected_at));
        let retry = host.retry.as_ref().unwrap().0;
        assert!(host.ensure_running(&root).is_err());
        assert_eq!(host.retry.as_ref().unwrap().0, retry);
        assert_eq!(fs::read(readings.join("poll-lease.json")).unwrap(), lease);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dropping_host_cleans_up_only_its_own_process_tree() {
        let path = std::env::temp_dir().join(format!("codexinfo-host-child-{}.txt", std::process::id()));
        let mut command = Command::new("node.exe");
        command.args(["-e", "const child=require('node:child_process').spawn(process.execPath,['-e','setInterval(()=>{},1000)'],{windowsHide:true,stdio:'ignore'});require('node:fs').writeFileSync(process.argv[1],String(child.pid));process.stdin.resume();"]);
        command.arg(&path);
        let process = HostProcess::spawn(command).unwrap();
        let started = Instant::now();
        while !path.exists() && started.elapsed() < Duration::from_secs(10) {
            thread::sleep(Duration::from_millis(25));
        }
        if !path.exists() { panic!("child fixture did not start: {}", process.stop().unwrap()); }
        let child_id = fs::read_to_string(&path).unwrap().parse::<u32>().unwrap();
        let raw = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, child_id) }.unwrap();
        let child = unsafe { OwnedHandle::from_raw_handle(raw.0) };
        drop(process);
        assert_eq!(unsafe { WaitForSingleObject(HANDLE(child.as_raw_handle()), 5000) }, WAIT_OBJECT_0);
        fs::remove_file(path).unwrap();
    }
}
