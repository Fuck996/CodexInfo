use chrono::Utc;
use std::{
    backtrace::Backtrace,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::Path,
    sync::{Mutex, OnceLock},
};

static LOG_FILE: OnceLock<Mutex<File>> = OnceLock::new();

pub fn initialize(directory: &Path) -> io::Result<()> {
    fs::create_dir_all(directory)?;
    let path = directory.join("lifecycle.log");
    if path.exists() && fs::metadata(&path)?.len() > 256 * 1024 {
        fs::copy(&path, directory.join("lifecycle.previous.log"))?;
        File::create(&path)?;
    }
    let file = OpenOptions::new().create(true).append(true).open(path)?;
    LOG_FILE
        .set(Mutex::new(file))
        .map_err(|_| io::Error::new(io::ErrorKind::AlreadyExists, "诊断日志已初始化"))?;

    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        record(&format!("panic: {info}\n{}", Backtrace::force_capture()));
        previous_hook(info);
    }));
    record(&format!("startup version={}", env!("CARGO_PKG_VERSION")));
    Ok(())
}

pub fn record(event: &str) {
    let entry = format!(
        "{} pid={} {event}",
        Utc::now().to_rfc3339(),
        std::process::id()
    );
    let Some(file) = LOG_FILE.get() else {
        eprintln!("{entry}");
        return;
    };
    match file.lock() {
        Ok(mut file) => {
            if let Err(error) = writeln!(file, "{entry}").and_then(|_| file.flush()) {
                eprintln!("写入诊断日志失败: {error}; {entry}");
            }
        }
        Err(error) => eprintln!("诊断日志锁异常: {error}; {entry}"),
    }
}
