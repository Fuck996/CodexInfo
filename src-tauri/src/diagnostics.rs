use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::{
    backtrace::Backtrace,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

#[cfg(target_os = "windows")]
#[path = "diagnostics_windows.rs"]
mod platform;

const MAX_LOG_BYTES: u64 = 512 * 1024;
static LOG_FILE: OnceLock<Mutex<RollingLog>> = OnceLock::new();
static SESSION: OnceLock<Mutex<Session>> = OnceLock::new();

pub(super) struct RollingLog {
    path: PathBuf,
    file: File,
    size: u64,
}

impl RollingLog {
    pub(super) fn open(path: PathBuf) -> io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let size = file.metadata()?.len();
        Ok(Self { path, file, size })
    }

    pub(super) fn write(&mut self, event: &str) -> io::Result<()> {
        let entry = format!(
            "{} pid={} {event}\n",
            Utc::now().to_rfc3339(),
            std::process::id()
        );
        if self.size + entry.len() as u64 > MAX_LOG_BYTES {
            self.file.flush()?;
            let previous = self.path.with_extension("previous.log");
            if previous.exists() {
                fs::remove_file(&previous)?;
            }
            fs::rename(&self.path, previous)?;
            self.file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)?;
            self.size = 0;
        }
        self.file.write_all(entry.as_bytes())?;
        self.file.flush()?;
        self.size += entry.len() as u64;
        Ok(())
    }
}

#[derive(Default, Deserialize, Serialize)]
pub(super) struct SessionStatus {
    pub pid: u32,
    pub creation_time: u64,
    pub version: String,
    pub started_at: String,
    pub phase: String,
    pub requested_exit: Option<String>,
    pub panic: bool,
    pub event_loop_exit: bool,
    pub monitor: String,
}

struct Session {
    path: PathBuf,
    status: SessionStatus,
}

impl Session {
    fn save(&self) -> io::Result<()> {
        let temporary = self.path.with_extension("json.tmp");
        fs::write(&temporary, serde_json::to_vec(&self.status)?)?;
        fs::rename(temporary, &self.path)
    }
}

pub(super) fn run_path(directory: &Path, pid: u32, creation_time: u64, extension: &str) -> PathBuf {
    directory.join(format!("monitor-{pid}-{creation_time}.{extension}"))
}

pub fn initialize(directory: &Path) -> io::Result<()> {
    fs::create_dir_all(directory)?;
    LOG_FILE
        .set(Mutex::new(RollingLog::open(
            directory.join("lifecycle.log"),
        )?))
        .map_err(|_| io::Error::new(io::ErrorKind::AlreadyExists, "诊断日志已初始化"))?;
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Some(session) = SESSION.get() {
            match session.try_lock() {
                Ok(mut session) => {
                    session.status.panic = true;
                    if let Err(error) = session.save() {
                        eprintln!("保存崩溃状态失败: {error}");
                    }
                }
                Err(error) => eprintln!("记录崩溃状态失败: {error}"),
            }
        }
        record(&format!("panic: {info}\n{}", Backtrace::force_capture()));
        previous_hook(info);
    }));
    record(&format!("startup version={}", env!("CARGO_PKG_VERSION")));
    #[cfg(target_os = "windows")]
    {
        let creation_time = platform::current_creation_time().map_err(io::Error::other)?;
        let status = SessionStatus {
            pid: std::process::id(),
            creation_time,
            version: env!("CARGO_PKG_VERSION").into(),
            started_at: Utc::now().to_rfc3339(),
            phase: "starting".into(),
            monitor: "starting".into(),
            ..Default::default()
        };
        let session = Session {
            path: run_path(directory, status.pid, creation_time, "json"),
            status,
        };
        session.save()?;
        SESSION
            .set(Mutex::new(session))
            .map_err(|_| io::Error::other("运行诊断已初始化"))?;
        if let Err(error) = platform::prune_old_runs(directory) {
            record(&format!("retention_error {error}"));
        }
        match platform::job_description() {
            Ok(description) => record(&format!("startup_job {description}")),
            Err(error) => record(&format!("startup_job_error {error}")),
        }
        let directory = directory.to_owned();
        std::thread::spawn(
            move || match platform::launch_monitor(&directory, creation_time) {
                Ok(pid) => {
                    update_session(|status| status.monitor = format!("active pid={pid}"));
                    record(&format!("monitor_started pid={pid}"));
                }
                Err(error) => {
                    update_session(|status| status.monitor = format!("failed: {error}"));
                    record(&format!("monitor_start_failed {error}"));
                }
            },
        );
    }
    Ok(())
}

fn update_session(update: impl FnOnce(&mut SessionStatus)) {
    let Some(session) = SESSION.get() else { return };
    match session.lock() {
        Ok(mut session) => {
            update(&mut session.status);
            if let Err(error) = session.save() {
                eprintln!("保存运行诊断失败: {error}");
            }
        }
        Err(error) => eprintln!("更新运行诊断失败: {error}"),
    }
}

pub fn ready() {
    update_session(|status| status.phase = "ready".into());
    record("event loop ready");
}

pub fn request_exit(reason: &str) {
    update_session(|status| status.requested_exit = Some(reason.into()));
    record(&format!("quit requested reason={reason}"));
}

pub fn event_loop_exit() {
    update_session(|status| {
        status.phase = "event_loop_exit".into();
        status.event_loop_exit = true;
    });
    record("event loop exit");
}

pub fn record(event: &str) {
    let Some(file) = LOG_FILE.get() else {
        eprintln!("{event}");
        return;
    };
    match file.lock() {
        Ok(mut file) => {
            if let Err(error) = file.write(event) {
                eprintln!("写入诊断日志失败: {error}; {event}");
            }
        }
        Err(error) => eprintln!("诊断日志锁异常: {error}; {event}"),
    }
}

#[tauri::command]
pub fn report_frontend_error(source: String, message: String) -> Result<(), String> {
    if !matches!(source.as_str(), "error" | "unhandledrejection") {
        return Err("未知前端错误来源".into());
    }
    let message: String = message.chars().take(4096).collect();
    record(&format!(
        "frontend_error {}",
        serde_json::json!({ "source": source, "message": message })
    ));
    Ok(())
}

pub fn handle_cli() -> bool {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--diagnostics-monitor")) {
        return false;
    }
    #[cfg(target_os = "windows")]
    if let Err(error) = platform::monitor_cli(args.collect()) {
        eprintln!("故障监控失败: {error}");
        std::process::exit(1);
    }
    true
}

pub(super) fn exit_kind(code: u32, status: &SessionStatus) -> &'static str {
    if status.panic {
        "panic"
    } else if code != 0 {
        "abnormal_exit"
    } else if status.requested_exit.is_some() {
        "requested_exit"
    } else if status.event_loop_exit {
        "event_loop_exit"
    } else {
        "unexpected_exit"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_exit_without_lifecycle_event_is_not_reported_as_normal() {
        assert_eq!(exit_kind(0, &SessionStatus::default()), "unexpected_exit");
    }

    #[test]
    fn nonzero_exit_overrides_a_menu_request() {
        let status = SessionStatus {
            requested_exit: Some("menu".into()),
            ..Default::default()
        };
        assert_eq!(exit_kind(0xc0000005, &status), "abnormal_exit");
        assert_eq!(exit_kind(0, &status), "requested_exit");
    }

    #[test]
    fn panic_and_event_loop_exit_remain_distinguishable() {
        let mut status = SessionStatus {
            panic: true,
            event_loop_exit: true,
            ..Default::default()
        };
        assert_eq!(exit_kind(0, &status), "panic");
        status.panic = false;
        assert_eq!(exit_kind(0, &status), "event_loop_exit");
    }

    #[test]
    fn rolling_log_keeps_recent_evidence_and_bounds_file_size() {
        let directory = std::env::temp_dir().join(format!(
            "codexinfo-log-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("lifecycle.log");
        let mut log = RollingLog::open(path.clone()).unwrap();
        let event = "x".repeat(200 * 1024);
        for _ in 0..6 {
            log.write(&event).unwrap();
        }
        log.write("last event").unwrap();
        drop(log);
        assert!(fs::metadata(&path).unwrap().len() <= MAX_LOG_BYTES);
        assert!(
            fs::metadata(path.with_extension("previous.log"))
                .unwrap()
                .len()
                <= MAX_LOG_BYTES
        );
        assert!(fs::read_to_string(path).unwrap().contains("last event"));
        fs::remove_dir_all(directory).unwrap();
    }
}
