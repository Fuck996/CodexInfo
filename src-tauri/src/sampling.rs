use super::{diagnostics, UsageProvider};
use std::{
    sync::{mpsc, Mutex},
    thread::{self, JoinHandle},
    time::Duration,
};

pub(super) struct Sampler {
    stop: mpsc::Sender<()>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl Sampler {
    pub(super) fn start(
        interval: Duration,
        mut read: impl FnMut(UsageProvider) -> Result<String, String> + Send + 'static,
    ) -> std::io::Result<Self> {
        let (stop, receiver) = mpsc::channel();
        let worker = thread::Builder::new().name("usage-sampler".into()).spawn(move || {
            diagnostics::record(&format!("usage_sampler started interval_ms={}", interval.as_millis()));
            let mut previous = [None, None];
            'sampling: loop {
                for (index, provider) in [UsageProvider::Codex, UsageProvider::Claude].into_iter().enumerate() {
                    if !matches!(receiver.try_recv(), Err(mpsc::TryRecvError::Empty)) { break 'sampling; }
                    let result = read(provider);
                    if previous[index].as_ref() != Some(&result) {
                        let message = match &result {
                            Ok(reading) => format!("status=ok {reading}"),
                            Err(error) => format!("status=error error={error}"),
                        };
                        diagnostics::record(&format!("usage_sampler provider={} {message}", provider.label()));
                        previous[index] = Some(result);
                    }
                }
                match receiver.recv_timeout(interval) {
                    Err(mpsc::RecvTimeoutError::Timeout) => {},
                    _ => break,
                }
            }
            diagnostics::record("usage_sampler stopped");
        })?;
        Ok(Self { stop, worker: Mutex::new(Some(worker)) })
    }

    pub(super) fn request_stop(&self) {
        let _ = self.stop.send(());
    }

    pub(super) fn shutdown(&self) -> Result<(), String> {
        self.request_stop();
        if let Some(worker) = self.worker.lock().map_err(|error| error.to_string())?.take() {
            worker.join().map_err(|_| "后台用量采样线程异常退出".to_string())?;
        }
        Ok(())
    }
}

impl Drop for Sampler {
    fn drop(&mut self) {
        if let Err(error) = self.shutdown() {
            diagnostics::record(&format!("usage_sampler shutdown_failed error={error}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_both_providers_immediately_and_repeatedly_without_a_window() {
        let (sender, receiver) = mpsc::channel();
        let sampler = Sampler::start(Duration::from_millis(10), move |provider| {
            sender.send(provider).unwrap();
            Ok("reading".into())
        }).unwrap();
        let readings = (0..4).map(|_| receiver.recv_timeout(Duration::from_secs(1)).unwrap()).collect::<Vec<_>>();
        sampler.shutdown().unwrap();
        assert_eq!(readings, [UsageProvider::Codex, UsageProvider::Claude, UsageProvider::Codex, UsageProvider::Claude]);
    }

    #[test]
    fn a_provider_failure_does_not_stop_the_other_provider_or_later_polls() {
        let (sender, receiver) = mpsc::channel();
        let sampler = Sampler::start(Duration::from_millis(10), move |provider| {
            sender.send(provider).unwrap();
            match provider {
                UsageProvider::Codex => Err("upstream failed".into()),
                UsageProvider::Claude => Ok("reading".into()),
            }
        }).unwrap();
        let readings = (0..4).map(|_| receiver.recv_timeout(Duration::from_secs(1)).unwrap()).collect::<Vec<_>>();
        sampler.shutdown().unwrap();
        assert_eq!(readings, [UsageProvider::Codex, UsageProvider::Claude, UsageProvider::Codex, UsageProvider::Claude]);
    }

    #[test]
    fn shutdown_wakes_a_sleeping_sampler_and_joins_the_worker() {
        let (sender, receiver) = mpsc::channel();
        let sampler = Sampler::start(Duration::from_secs(3600), move |provider| {
            sender.send(provider).unwrap();
            Ok("reading".into())
        }).unwrap();
        for _ in 0..2 { receiver.recv_timeout(Duration::from_secs(1)).unwrap(); }
        sampler.shutdown().unwrap();
        assert!(sampler.worker.lock().unwrap().is_none());
        assert!(receiver.recv_timeout(Duration::from_millis(50)).is_err());
        sampler.shutdown().unwrap();
    }
}
