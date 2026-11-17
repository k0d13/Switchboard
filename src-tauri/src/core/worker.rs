//! A background thread that can be told to stop, and can sleep in a way that wakes up on stop.

use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use parking_lot::{Condvar, Mutex};

#[derive(Default)]
struct Flag {
    stopped: Mutex<bool>,
    cond: Condvar,
}

/// Handed to the worker's closure.
#[derive(Clone)]
pub struct StopToken(Arc<Flag>);

impl StopToken {
    pub fn stopped(&self) -> bool {
        *self.0.stopped.lock()
    }

    /// Sleeps for `dur`, or less if stop is requested. Returns true if it should stop.
    pub fn sleep(&self, dur: Duration) -> bool {
        let mut stopped = self.0.stopped.lock();
        if !*stopped {
            self.0.cond.wait_for(&mut stopped, dur);
        }
        *stopped
    }
}

pub struct Worker {
    token: StopToken,
    handle: Option<JoinHandle<()>>,
}

impl Worker {
    pub fn spawn(name: &str, f: impl FnOnce(StopToken) + Send + 'static) -> Self {
        let token = StopToken(Arc::default());
        let t = token.clone();
        let handle = std::thread::Builder::new().name(name.into()).spawn(move || f(t)).expect("spawn worker thread");
        Self { token, handle: Some(handle) }
    }

    /// Asks the worker to stop without waiting for it.
    pub fn signal(&self) {
        *self.token.0.stopped.lock() = true;
        self.token.0.cond.notify_all();
    }

    /// Asks the worker to stop and waits for it to finish.
    pub fn stop(mut self) {
        self.signal();
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.signal();
    }
}
