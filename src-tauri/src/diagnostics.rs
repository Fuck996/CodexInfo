use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::{
    backtrace::Backtrace,
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant, SystemTime},
};

#[cfg(target_os = "windows")]
#[path = "diagnostics_windows.rs"]
mod platform;

const MAX_LOG_BYTES: u64 = 512 * 1024;
const MAX_DIRECTORY_BYTES: u64 = 8 * 1024 * 1024;
const MAX_COMPLETED_RUNS: usize = 12;
const MAX_RUN_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const ERROR_INTERVAL: Duration = Duration::from_secs(60);
static LOG_FILE: OnceLock<Mutex<RollingLog>> = OnceLock::new();
static SESSION: OnceLock<Mutex<Session>> = OnceLock::new();
static DOCK_STATUS: Mutex<Option<String>> = Mutex::new(None);
static FRONTEND_ERRORS: Mutex<ErrorBatch> = Mutex::new(ErrorBatch {
    last_written: None,
    pending: 0,
    last_event: String::new(),
});

#[derive(Default)]
struct ErrorBatch {
    last_written: Option<Instant>,
    pending: u64,
    last_event: String,
}

impl ErrorBatch {
    fn push(&mut self, event: String, now: Instant) -> Option<String> {
        self.pending = self.pending.saturating_add(1);
        self.last_event = event;
        self.take(now, false)
    }

    fn take(&mut self, now: Instant, force: bool) -> Option<String> {
        if self.pending == 0
            || (!force
                && self
                    .last_written
                    .is_some_and(|last| now.duration_since(last) < ERROR_INTERVAL))
        {
            return None;
        }
        let event = format!("{} occurrences={}", self.last_event, self.pending);
        self.pending = 0;
        self.last_written = Some(now);
        Some(event)
    }
}

fn directory_lock(directory: &Path) -> io::Result<File> {
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.join(".retention.lock"))?;
    lock.lock()?;
    Ok(lock)
}

struct RetainedRun {
    files: Vec<PathBuf>,
    size: u64,
    modified: SystemTime,
    active: bool,
}

fn monitor_file(name: &str) -> Option<(u32, u64)> {
    let (stem, extension) = name.strip_prefix("monitor-")?.split_once('.')?;
    if !matches!(
        extension,
        "log" | "previous.log" | "json" | "json.tmp" | "ready" | "ready.tmp"
    ) {
        return None;
    }
    let (pid, created) = stem.split_once('-')?;
    Some((pid.parse().ok()?, created.parse().ok()?))
}

// 调用方持有目录锁；运行标识必须核对进程创建时间，不能只相信遗留的 ready 文件。
fn maintain_retention(directory: &Path, reserve: u64, protected: Option<&Path>) -> io::Result<()> {
    prune_runs(
        directory,
        reserve,
        protected,
        SystemTime::now(),
        |pid, created| {
            #[cfg(target_os = "windows")]
            return platform::run_is_active(directory, pid, created).map_err(io::Error::other);
            #[cfg(not(target_os = "windows"))]
            Ok(pid == std::process::id())
        },
    )
}

fn prune_runs(
    directory: &Path,
    reserve: u64,
    protected: Option<&Path>,
    now: SystemTime,
    is_active: impl Fn(u32, u64) -> io::Result<bool>,
) -> io::Result<()> {
    let mut runs: BTreeMap<(u32, u64), RetainedRun> = BTreeMap::new();
    let mut total = 0;
    let mut previous_lifecycle = None;
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let identity = monitor_file(name);
        if identity.is_none() && !matches!(name, "lifecycle.log" | "lifecycle.previous.log") {
            continue;
        }
        let metadata = entry.metadata()?;
        if !metadata.is_file() {
            continue;
        }
        total += metadata.len();
        if let Some(identity) = identity {
            let run = runs.entry(identity).or_insert(RetainedRun {
                files: Vec::new(),
                size: 0,
                modified: SystemTime::UNIX_EPOCH,
                active: false,
            });
            run.size += metadata.len();
            run.modified = run.modified.max(metadata.modified()?);
            run.active |= protected == Some(path.as_path());
            run.files.push(path);
        } else if name == "lifecycle.previous.log" {
            previous_lifecycle = Some((path, metadata.len()));
        }
    }
    let mut completed = Vec::new();
    for ((pid, created), mut run) in runs {
        run.active |= is_active(pid, created)?;
        if !run.active {
            completed.push(run);
        }
    }
    completed.sort_by_key(|run| std::cmp::Reverse(run.modified));
    let mut kept = Vec::new();
    for (index, run) in completed.into_iter().enumerate() {
        if index >= MAX_COMPLETED_RUNS
            || now
                .duration_since(run.modified)
                .is_ok_and(|age| age > MAX_RUN_AGE)
        {
            remove_run(&run)?;
            total -= run.size;
        } else {
            kept.push(run);
        }
    }
    while total.saturating_add(reserve) > MAX_DIRECTORY_BYTES {
        if let Some(run) = kept.pop() {
            remove_run(&run)?;
            total -= run.size;
        } else if let Some((path, size)) = previous_lifecycle.take() {
            fs::remove_file(path)?;
            total -= size;
        } else {
            return Err(io::Error::other(
                "诊断目录已达 8 MiB 上限，活动记录不能删除",
            ));
        }
    }
    Ok(())
}

fn remove_run(run: &RetainedRun) -> io::Result<()> {
    for path in &run.files {
        fs::remove_file(path)?;
    }
    Ok(())
}

pub(super) fn write_metadata(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let directory = path
        .parent()
        .ok_or_else(|| io::Error::other("诊断目录无效"))?;
    let _lock = directory_lock(directory)?;
    let temporary = path.with_extension(format!(
        "{}.tmp",
        path.extension().unwrap_or_default().to_string_lossy()
    ));
    maintain_retention(directory, bytes.len() as u64, Some(path))?;
    fs::write(&temporary, bytes)?;
    fs::rename(temporary, path)
}

pub(super) fn remove_metadata(path: &Path) -> io::Result<()> {
    let directory = path
        .parent()
        .ok_or_else(|| io::Error::other("诊断目录无效"))?;
    let _lock = directory_lock(directory)?;
    fs::remove_file(path)
}

pub(super) struct RollingLog {
    path: PathBuf,
}

impl RollingLog {
    pub(super) fn open(path: PathBuf) -> io::Result<Self> {
        let directory = path
            .parent()
            .ok_or_else(|| io::Error::other("诊断目录无效"))?;
        let _lock = directory_lock(directory)?;
        maintain_retention(directory, 0, Some(&path))?;
        OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self { path })
    }

    pub(super) fn write(&mut self, event: &str) -> io::Result<()> {
        let mut entry = format!(
            "{} pid={} {event}\n",
            Utc::now().to_rfc3339(),
            std::process::id()
        );
        if entry.len() as u64 > MAX_LOG_BYTES {
            let mut end = MAX_LOG_BYTES as usize - 32;
            while !entry.is_char_boundary(end) {
                end -= 1;
            }
            entry.truncate(end);
            entry.push_str("\n[event truncated]\n");
        }
        let directory = self
            .path
            .parent()
            .ok_or_else(|| io::Error::other("诊断目录无效"))?;
        let _lock = directory_lock(directory)?;
        let size = fs::metadata(&self.path)?.len();
        maintain_retention(directory, entry.len() as u64, Some(&self.path))?;
        if size + entry.len() as u64 > MAX_LOG_BYTES {
            let previous = self.path.with_extension("previous.log");
            if previous.exists() {
                fs::remove_file(&previous)?;
            }
            fs::rename(&self.path, previous)?;
        }
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?
            .write_all(entry.as_bytes())?;
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
        write_metadata(&self.path, &serde_json::to_vec(&self.status)?)
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
    std::thread::Builder::new()
        .name("diagnostic-errors".into())
        .spawn(|| loop {
            std::thread::sleep(ERROR_INTERVAL);
            flush_frontend_errors(false);
            if SESSION.get().is_some_and(|session| {
                session
                    .lock()
                    .is_ok_and(|session| session.status.event_loop_exit)
            }) {
                break;
            }
        })?;
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
    flush_frontend_errors(true);
    update_session(|status| status.requested_exit = Some(reason.into()));
    record(&format!("quit requested reason={reason}"));
}

pub fn event_loop_exit() {
    flush_frontend_errors(true);
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

pub fn dock_status(state: &str, detail: &str) {
    match DOCK_STATUS.lock() {
        Ok(mut previous) if previous.as_deref() != Some(state) => {
            record(&format!("dock_state state={state} {detail}"));
            *previous = Some(state.into());
        }
        Ok(_) => {}
        Err(error) => eprintln!("悬浮窗诊断锁异常: {error}"),
    }
}

#[tauri::command]
pub fn report_frontend_error(source: String, message: String) -> Result<(), String> {
    if !matches!(source.as_str(), "error" | "unhandledrejection") {
        return Err("未知前端错误来源".into());
    }
    let message: String = message.chars().take(4096).collect();
    let event = format!(
        "frontend_error {}",
        serde_json::json!({ "source": source, "message": message })
    );
    let mut errors = FRONTEND_ERRORS.lock().map_err(|error| error.to_string())?;
    if let Some(event) = errors.push(event, Instant::now()) {
        record(&event);
    }
    Ok(())
}

fn flush_frontend_errors(force: bool) {
    match FRONTEND_ERRORS.lock() {
        Ok(mut errors) => {
            if let Some(event) = errors.take(Instant::now(), force) {
                record(&event);
            }
        }
        Err(error) => eprintln!("前端错误汇总锁异常: {error}"),
    }
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

    fn temporary_directory(label: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "codexinfo-{label}-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        fs::create_dir(&directory).unwrap();
        directory
    }

    #[test]
    fn retention_bounds_count_and_age_including_orphan_files_but_preserves_active_runs() {
        let directory = temporary_directory("retention");
        let now = SystemTime::now();
        for index in 1..=15 {
            let path = run_path(&directory, index, 1, "log");
            fs::write(&path, "evidence").unwrap();
            File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_modified(now - Duration::from_secs(index as u64 * 60))
                .unwrap();
        }
        let expired = run_path(&directory, 90, 1, "ready");
        fs::write(&expired, "1234").unwrap();
        File::options()
            .write(true)
            .open(&expired)
            .unwrap()
            .set_modified(now - MAX_RUN_AGE - Duration::from_secs(1))
            .unwrap();
        let active = run_path(&directory, 99, 1, "log");
        fs::write(&active, "active evidence").unwrap();
        File::options()
            .write(true)
            .open(&active)
            .unwrap()
            .set_modified(now - MAX_RUN_AGE * 2)
            .unwrap();
        prune_runs(&directory, 0, None, now, |pid, _| Ok(pid == 99)).unwrap();
        assert!(active.exists());
        assert!(!expired.exists());
        assert!(run_path(&directory, 12, 1, "log").exists());
        assert!(!run_path(&directory, 13, 1, "log").exists());
        assert_eq!(
            fs::read_dir(&directory).unwrap().count(),
            MAX_COMPLETED_RUNS + 1
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn directory_quota_reserves_space_for_atomic_metadata_writes() {
        let directory = temporary_directory("quota");
        let now = SystemTime::now();
        for index in 1..=10 {
            let path = run_path(&directory, index, 1, "log");
            File::create(&path).unwrap().set_len(1024 * 1024).unwrap();
            File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_modified(now - Duration::from_secs(index as u64))
                .unwrap();
        }
        prune_runs(&directory, 4096, None, now, |_, _| Ok(false)).unwrap();
        let total: u64 = fs::read_dir(&directory)
            .unwrap()
            .map(|entry| entry.unwrap().metadata().unwrap().len())
            .sum();
        assert!(total + 4096 <= MAX_DIRECTORY_BYTES);
        assert!(run_path(&directory, 1, 1, "log").exists());
        assert!(!run_path(&directory, 10, 1, "log").exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn frontend_error_storm_is_coalesced_without_losing_the_count() {
        let mut errors = ErrorBatch::default();
        let start = Instant::now();
        assert!(errors
            .push("first".into(), start)
            .unwrap()
            .contains("occurrences=1"));
        for _ in 0..1000 {
            assert!(errors
                .push("repeated".into(), start + Duration::from_secs(1))
                .is_none());
        }
        assert!(errors
            .take(start + Duration::from_secs(59), false)
            .is_none());
        assert!(errors
            .take(start + Duration::from_secs(60), false)
            .unwrap()
            .contains("repeated occurrences=1000"));
        assert!(errors.take(start + Duration::from_secs(61), true).is_none());
        errors.push("last".into(), start + Duration::from_secs(61));
        assert!(errors
            .take(start + Duration::from_secs(62), true)
            .unwrap()
            .contains("last"));
    }

    #[test]
    fn oversized_unicode_event_still_obeys_segment_limit() {
        let directory = temporary_directory("unicode-log");
        let path = directory.join("lifecycle.log");
        let mut log = RollingLog::open(path.clone()).unwrap();
        log.write(&"故障".repeat(MAX_LOG_BYTES as usize)).unwrap();
        let bytes = fs::read(&path).unwrap();
        assert!(bytes.len() as u64 <= MAX_LOG_BYTES);
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.ends_with("[event truncated]\n"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn independent_writers_share_rotation_without_writing_into_retired_segments() {
        let directory = temporary_directory("shared-log");
        let path = directory.join("lifecycle.log");
        let workers: Vec<_> = (0..4)
            .map(|_| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let mut log = RollingLog::open(path).unwrap();
                    for _ in 0..200 {
                        log.write(&"x".repeat(2048)).unwrap();
                    }
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
        RollingLog::open(path.clone())
            .unwrap()
            .write("last shared event")
            .unwrap();
        assert!(fs::read_to_string(&path)
            .unwrap()
            .contains("last shared event"));
        assert!(fs::metadata(&path).unwrap().len() <= MAX_LOG_BYTES);
        assert!(
            fs::metadata(path.with_extension("previous.log"))
                .unwrap()
                .len()
                <= MAX_LOG_BYTES
        );
        fs::remove_dir_all(directory).unwrap();
    }

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
