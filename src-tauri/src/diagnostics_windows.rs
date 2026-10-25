use super::{exit_kind, remove_metadata, run_path, write_metadata, RollingLog, SessionStatus};
use std::{
    ffi::OsString,
    fs,
    mem::size_of,
    os::windows::process::CommandExt,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use windows::{
    core::{BOOL, HRESULT},
    Win32::{
        Foundation::{
            CloseHandle, FILETIME, HANDLE, HWND, LPARAM, WAIT_OBJECT_0, WAIT_TIMEOUT, WPARAM,
        },
        System::{
            JobObjects::{
                IsProcessInJob, JobObjectExtendedLimitInformation, QueryInformationJobObject,
                JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            },
            ProcessStatus::{
                GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
            },
            Threading::{
                GetCurrentProcess, GetExitCodeProcess, GetGuiResources, GetProcessHandleCount,
                GetProcessTimes, OpenProcess, WaitForSingleObject, GR_GDIOBJECTS, GR_USEROBJECTS,
                PROCESS_QUERY_INFORMATION, PROCESS_SYNCHRONIZE,
            },
        },
        UI::WindowsAndMessaging::{
            EnumWindows, GetWindowThreadProcessId, SendMessageTimeoutW, SMTO_ABORTIFHUNG,
            SMTO_BLOCK, SMTO_ERRORONEXIT, WM_NULL,
        },
    },
};

const KILL_ON_JOB_CLOSE: u32 = 0x2000;
const SAMPLE_INTERVAL: Duration = Duration::from_secs(30);
const HEALTHY_LOG_INTERVAL: Duration = Duration::from_secs(10 * 60);

#[derive(serde::Serialize, serde::Deserialize)]
struct MonitorReady {
    pid: u32,
    creation_time: u64,
}

#[derive(Clone, PartialEq, Eq)]
enum SampleHealth {
    Windows { unresponsive: usize },
    Error(String),
}

#[derive(Default)]
struct SampleSchedule {
    health: Option<SampleHealth>,
    written_at: Option<Instant>,
}

impl SampleSchedule {
    fn should_write(&mut self, now: Instant, health: SampleHealth) -> bool {
        if self.health.as_ref() == Some(&health)
            && self
                .written_at
                .is_some_and(|last| now.duration_since(last) < HEALTHY_LOG_INTERVAL)
        {
            return false;
        }
        self.health = Some(health);
        self.written_at = Some(now);
        true
    }
}

struct Process(HANDLE);

impl Drop for Process {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

impl Process {
    fn open(pid: u32) -> windows::core::Result<Self> {
        unsafe {
            OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_SYNCHRONIZE, false, pid).map(Self)
        }
    }

    fn creation_time(&self) -> Result<u64, String> {
        creation_time(self.0)
    }

    fn exit_code(&self) -> Result<u32, String> {
        let mut code = 0;
        unsafe { GetExitCodeProcess(self.0, &mut code) }.map_err(|error| error.to_string())?;
        Ok(code)
    }
}

fn creation_time(process: HANDLE) -> Result<u64, String> {
    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    unsafe { GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user) }
        .map_err(|error| error.to_string())?;
    Ok((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
}

pub(super) fn current_creation_time() -> Result<u64, String> {
    creation_time(unsafe { GetCurrentProcess() })
}

fn job_flags() -> Result<Option<u32>, String> {
    let mut in_job = BOOL::default();
    unsafe { IsProcessInJob(GetCurrentProcess(), None, &mut in_job) }
        .map_err(|error| error.to_string())?;
    if !in_job.as_bool() {
        return Ok(None);
    }
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    unsafe {
        QueryInformationJobObject(
            None,
            JobObjectExtendedLimitInformation,
            (&mut limits as *mut JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            None,
        )
    }
    .map_err(|error| error.to_string())?;
    Ok(Some(limits.BasicLimitInformation.LimitFlags.0))
}

pub(super) fn job_description() -> Result<String, String> {
    Ok(match job_flags()? {
        Some(flags) => format!(
            "in_job=true flags=0x{flags:x} kill_on_close={}",
            flags & KILL_ON_JOB_CLOSE != 0
        ),
        None => "in_job=false kill_on_close=false".into(),
    })
}

fn powershell_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

pub(super) fn launch_monitor(directory: &Path, created: u64) -> Result<u32, String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let pid = std::process::id();
    let command_line = format!(
        "\"{}\" --diagnostics-monitor {pid} {created} \"{}\"",
        executable.display(),
        directory.display()
    );
    let script = format!(
        "$ErrorActionPreference='Stop'; \
         $startup=New-CimInstance -ClassName Win32_ProcessStartup -ClientOnly -Property @{{CreateFlags=[uint32]16777216;ShowWindow=[uint16]0}}; \
         $result=Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{{CommandLine={};ProcessStartupInformation=$startup}}; \
         $result | Select-Object ReturnValue,ProcessId | ConvertTo-Json -Compress",
        powershell_literal(&command_line)
    );
    let system_root = std::env::var_os("SystemRoot").ok_or("缺少 SystemRoot")?;
    let powershell = Path::new(&system_root).join("System32/WindowsPowerShell/v1.0/powershell.exe");
    // WMI 从系统服务创建监控进程，避免它与组件一起被启动宿主的 Job 终止。
    let mut child = Command::new(powershell)
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &script,
        ])
        .creation_flags(0x08000000)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("启动监控代理失败: {error}"))?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < Duration::from_secs(12) => {
                thread::sleep(Duration::from_millis(100))
            }
            pending => {
                let reason = match pending {
                    Err(error) => error.to_string(),
                    _ => "WMI 启动超时".into(),
                };
                child
                    .kill()
                    .map_err(|error| format!("{reason}; 停止代理失败: {error}"))?;
                child
                    .wait()
                    .map_err(|error| format!("等待监控代理退出失败: {error}"))?;
                return Err(reason);
            }
        }
    }
    let output = child
        .wait_with_output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "监控代理失败: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct LaunchResult {
        return_value: u32,
        process_id: u32,
    }
    let result: LaunchResult = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("监控启动响应无效: {error}"))?;
    if result.return_value != 0 {
        return Err(format!("WMI 创建监控失败，返回码 {}", result.return_value));
    }
    let ready = run_path(directory, pid, created, "ready");
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(6) {
        match fs::read_to_string(&ready) {
            Ok(value) => {
                let ready: MonitorReady = serde_json::from_str(&value)
                    .map_err(|error| format!("监控就绪标识无效: {error}"))?;
                let monitor =
                    Process::open(result.process_id).map_err(|error| error.to_string())?;
                if ready.pid != result.process_id
                    || ready.creation_time != monitor.creation_time()?
                {
                    return Err("监控就绪标识与启动进程不一致".into());
                }
                return Ok(result.process_id);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                thread::sleep(Duration::from_millis(100))
            }
            Err(error) => return Err(format!("读取监控就绪标识失败: {error}")),
        }
    }
    Err(format!(
        "监控未就绪，检查 {}",
        run_path(directory, pid, created, "log").display()
    ))
}

struct WindowProbe {
    pid: u32,
    windows: Vec<HWND>,
}

unsafe extern "system" fn collect_window(hwnd: HWND, data: LPARAM) -> BOOL {
    let probe = unsafe { &mut *(data.0 as *mut WindowProbe) };
    let mut pid = 0;
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
    }
    if pid == probe.pid {
        probe.windows.push(hwnd);
    }
    BOOL(1)
}

fn sample(process: &Process, pid: u32) -> Result<(String, SampleHealth), String> {
    let mut memory = PROCESS_MEMORY_COUNTERS_EX {
        cb: size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        ..Default::default()
    };
    let mut handles = 0;
    unsafe {
        GetProcessMemoryInfo(
            process.0,
            (&mut memory as *mut PROCESS_MEMORY_COUNTERS_EX).cast::<PROCESS_MEMORY_COUNTERS>(),
            size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        )
        .map_err(|error| error.to_string())?;
        GetProcessHandleCount(process.0, &mut handles).map_err(|error| error.to_string())?;
    }
    let mut probe = WindowProbe {
        pid,
        windows: Vec::new(),
    };
    unsafe {
        EnumWindows(
            Some(collect_window),
            LPARAM((&mut probe as *mut WindowProbe) as isize),
        )
    }
    .map_err(|error| error.to_string())?;
    let mut unresponsive = 0;
    for hwnd in &probe.windows {
        let result = unsafe {
            SendMessageTimeoutW(
                *hwnd,
                WM_NULL,
                WPARAM(0),
                LPARAM(0),
                SMTO_ABORTIFHUNG | SMTO_BLOCK | SMTO_ERRORONEXIT,
                1000,
                None,
            )
        };
        if result.0 == 0 {
            unresponsive += 1;
        }
    }
    let event = format!("heartbeat target_pid={pid} private_bytes={} working_set_bytes={} handles={handles} gdi={} user={} windows={} unresponsive_windows={unresponsive}",
        memory.PrivateUsage, memory.WorkingSetSize, unsafe { GetGuiResources(process.0, GR_GDIOBJECTS) },
        unsafe { GetGuiResources(process.0, GR_USEROBJECTS) }, probe.windows.len());
    Ok((event, SampleHealth::Windows { unresponsive }))
}

pub(super) fn monitor_cli(args: Vec<OsString>) -> Result<(), String> {
    if args.len() != 3 {
        return Err("监控参数应为进程 ID、创建时间和日志目录".into());
    }
    let pid: u32 = args[0]
        .to_str()
        .ok_or("进程 ID 编码无效")?
        .parse()
        .map_err(|_| "进程 ID 无效")?;
    let created: u64 = args[1]
        .to_str()
        .ok_or("创建时间编码无效")?
        .parse()
        .map_err(|_| "创建时间无效")?;
    let directory = Path::new(&args[2]);
    let mut log = RollingLog::open(run_path(directory, pid, created, "log"))
        .map_err(|error| error.to_string())?;
    let result = monitor(&mut log, directory, pid, created);
    if let Err(error) = &result {
        log.write(&format!("monitor_error {error}"))
            .map_err(|write_error| format!("{error}; 写日志失败: {write_error}"))?;
    }
    result
}

fn monitor(log: &mut RollingLog, directory: &Path, pid: u32, created: u64) -> Result<(), String> {
    let process = Process::open(pid).map_err(|error| format!("打开目标进程失败: {error}"))?;
    if process.creation_time()? != created {
        return Err("目标进程 ID 已被复用，停止监控".into());
    }
    let job = job_flags()?;
    if job.is_some_and(|flags| flags & KILL_ON_JOB_CLOSE != 0) {
        return Err("监控仍受随宿主关闭终止的 Job 管理，不能提供独立退出监控".into());
    }
    log.write(&format!(
        "monitor_ready target_pid={pid} target_creation_time={created} {}",
        job_description()?
    ))
    .map_err(|error| error.to_string())?;
    let ready = run_path(directory, pid, created, "ready");
    let marker = MonitorReady {
        pid: std::process::id(),
        creation_time: current_creation_time()?,
    };
    write_metadata(
        &ready,
        &serde_json::to_vec(&marker).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    let result = watch_process(log, &process, directory, pid, created);
    remove_metadata(&ready).map_err(|error| format!("清理监控就绪标识失败: {error}"))?;
    result
}

fn watch_process(
    log: &mut RollingLog,
    process: &Process,
    directory: &Path,
    pid: u32,
    created: u64,
) -> Result<(), String> {
    let mut sampled_at: Option<Instant> = None;
    let mut schedule = SampleSchedule::default();
    let mut last_sample = None;
    loop {
        match unsafe { WaitForSingleObject(process.0, 5000) } {
            WAIT_OBJECT_0 => break,
            WAIT_TIMEOUT => {
                if sampled_at.is_none_or(|last| last.elapsed() >= SAMPLE_INTERVAL) {
                    let (event, health) = match sample(process, pid) {
                        Ok(sample) => sample,
                        Err(error) => (
                            format!("sample_error target_pid={pid} {error}"),
                            SampleHealth::Error(error),
                        ),
                    };
                    if schedule.should_write(Instant::now(), health) {
                        log.write(&event).map_err(|error| error.to_string())?;
                    }
                    last_sample = Some(event);
                    sampled_at = Some(Instant::now());
                }
            }
            result => {
                return Err(format!(
                    "等待目标进程失败: {:?}; {}",
                    result,
                    windows::core::Error::from_win32()
                ))
            }
        }
    }
    let code = process.exit_code()?;
    let state_path = run_path(directory, pid, created, "json");
    let state = fs::read(&state_path)
        .map_err(|error| error.to_string())
        .and_then(|bytes| {
            serde_json::from_slice::<SessionStatus>(&bytes).map_err(|error| error.to_string())
        });
    let classification = match state {
        Ok(status) => format!(
            "kind={} phase={} requested_exit={:?} panic={} event_loop_exit={}",
            exit_kind(code, &status),
            status.phase,
            status.requested_exit,
            status.panic,
            status.event_loop_exit
        ),
        Err(error) => format!("kind=unknown state_error={error:?}"),
    };
    log.write(&format!(
        "process_exit target_pid={pid} exit_code={code} exit_hex=0x{code:08x} {classification} last_sample_age_secs={:?} last_sample={last_sample:?}",
        sampled_at.map(|time| time.elapsed().as_secs())
    ))
    .map_err(|error| error.to_string())
}

fn process_matches(pid: u32, created: u64) -> Result<bool, String> {
    match Process::open(pid) {
        Ok(process) => Ok(process.creation_time()? == created
            && unsafe { WaitForSingleObject(process.0, 0) } == WAIT_TIMEOUT),
        Err(error) if error.code() == HRESULT::from_win32(87) => Ok(false),
        Err(error) => Err(format!("检查诊断进程失败 pid={pid}: {error}")),
    }
}

pub(super) fn run_is_active(directory: &Path, pid: u32, created: u64) -> Result<bool, String> {
    if process_matches(pid, created)? {
        return Ok(true);
    }
    let ready = run_path(directory, pid, created, "ready");
    match fs::read(&ready) {
        Ok(bytes) => match serde_json::from_slice::<MonitorReady>(&bytes) {
            Ok(monitor) => process_matches(monitor.pid, monitor.creation_time),
            // 旧版本只写 PID；没有创建时间的标识不能证明监控进程仍然存活。
            Err(_)
                if String::from_utf8_lossy(&bytes)
                    .trim()
                    .parse::<u32>()
                    .is_ok() =>
            {
                Ok(false)
            }
            Err(error) => Err(format!("监控标识损坏 {}: {error}", ready.display())),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("读取旧监控标识失败: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn healthy_samples_write_every_ten_minutes_but_health_changes_write_immediately() {
        let mut schedule = SampleSchedule::default();
        let start = Instant::now();
        let healthy = SampleHealth::Windows { unresponsive: 0 };
        assert!(schedule.should_write(start, healthy.clone()));
        for seconds in (30..600).step_by(30) {
            assert!(!schedule.should_write(start + Duration::from_secs(seconds), healthy.clone()));
        }
        assert!(schedule.should_write(start + Duration::from_secs(600), healthy.clone()));
        assert!(schedule.should_write(
            start + Duration::from_secs(630),
            SampleHealth::Windows { unresponsive: 1 }
        ));
        assert!(!schedule.should_write(
            start + Duration::from_secs(660),
            SampleHealth::Windows { unresponsive: 1 }
        ));
        assert!(schedule.should_write(start + Duration::from_secs(690), healthy));
        assert!(schedule.should_write(
            start + Duration::from_secs(720),
            SampleHealth::Error("access denied".into())
        ));
    }

    #[test]
    fn stale_ready_markers_and_reused_pids_do_not_protect_dead_runs() {
        let directory = std::env::temp_dir().join(format!(
            "codexinfo-ready-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        ));
        fs::create_dir(&directory).unwrap();
        let created = current_creation_time().unwrap();
        let pid = std::process::id();
        assert!(run_is_active(&directory, pid, created).unwrap());
        assert!(!run_is_active(&directory, pid, created - 1).unwrap());
        let path = run_path(&directory, pid, created - 1, "ready");
        fs::write(&path, pid.to_string()).unwrap();
        assert!(!run_is_active(&directory, pid, created - 1).unwrap());
        let marker = MonitorReady {
            pid,
            creation_time: created,
        };
        fs::write(&path, serde_json::to_vec(&marker).unwrap()).unwrap();
        assert!(run_is_active(&directory, pid, created - 1).unwrap());
        fs::remove_dir_all(directory).unwrap();
    }

    fn exercise_exit(forced: bool) -> String {
        let directory = std::env::temp_dir().join(format!(
            "codexinfo-monitor-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        ));
        fs::create_dir(&directory).unwrap();
        let shell = Path::new(&std::env::var_os("SystemRoot").unwrap())
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let sleep = if forced {
            "Start-Sleep -Seconds 30"
        } else {
            "Start-Sleep -Milliseconds 300"
        };
        let mut child = Command::new(shell)
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                sleep,
            ])
            .creation_flags(0x08000000)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        let process = Process::open(pid).unwrap();
        let created = process.creation_time().unwrap();
        let status = SessionStatus {
            pid,
            creation_time: created,
            requested_exit: if forced { None } else { Some("menu".into()) },
            ..Default::default()
        };
        fs::write(
            run_path(&directory, pid, created, "json"),
            serde_json::to_vec(&status).unwrap(),
        )
        .unwrap();
        let worker = thread::spawn(move || {
            if forced {
                thread::sleep(Duration::from_millis(300));
                child.kill().unwrap();
            }
            child.wait().unwrap()
        });
        let log_path = directory.join("test.log");
        let mut log = RollingLog::open(log_path.clone()).unwrap();
        watch_process(&mut log, &process, &directory, pid, created).unwrap();
        let exit = worker.join().unwrap();
        assert_eq!(exit.success(), !forced);
        drop(log);
        let output = fs::read_to_string(log_path).unwrap();
        fs::remove_dir_all(directory).unwrap();
        output
    }

    #[test]
    fn monitor_records_forced_termination_after_target_cannot_log() {
        let output = exercise_exit(true);
        assert!(output.contains("process_exit"));
        assert!(output.contains("kind=abnormal_exit"));
        assert!(output.contains("exit_hex=0x00000001"));
    }

    #[test]
    fn monitor_distinguishes_requested_zero_exit() {
        let output = exercise_exit(false);
        assert!(output.contains("exit_hex=0x00000000"));
        assert!(output.contains("kind=requested_exit"));
    }

    #[test]
    fn powershell_paths_preserve_apostrophes_without_executing_them() {
        assert_eq!(
            powershell_literal("C:\\O'Brien\\widget.exe"),
            "'C:\\O''Brien\\widget.exe'"
        );
    }
}
