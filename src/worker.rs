//! Runs libsm64 on its own thread so a hang in SM64 code can never freeze the game.
//!
//! The game thread sends jobs and waits at most `TIMEOUT` for each. If a job doesn't come
//! back in time, libsm64 is declared hung: every later call returns `None` right away and
//! the mod switches Mario mode off (the stuck thread is simply abandoned).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use crate::log;
use crate::sm64::Geometry;

const TIMEOUT: Duration = Duration::from_millis(100);

pub struct Ctx {
    pub geo: Geometry,
}

type Job = Box<dyn FnOnce(&mut Ctx) + Send>;

static SENDER: OnceLock<Mutex<Sender<Job>>> = OnceLock::new();
static HUNG: AtomicBool = AtomicBool::new(false);

fn sender() -> &'static Mutex<Sender<Job>> {
    SENDER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Job>();
        std::thread::spawn(move || {
            let mut ctx = Ctx { geo: Geometry::new() };
            while let Ok(job) = rx.recv() {
                crate::skate::apply();
                job(&mut ctx);
                crate::skate::publish();
            }
        });
        Mutex::new(tx)
    })
}

pub fn hung() -> bool {
    HUNG.load(Ordering::Relaxed)
}

/// Runs `f` on the libsm64 thread and waits for its result (or gives up after `TIMEOUT`).
pub fn call<R: Send + 'static>(name: &'static str, f: impl FnOnce(&mut Ctx) -> R + Send + 'static) -> Option<R> {
    call_timeout(name, TIMEOUT, f)
}

pub fn call_timeout<R: Send + 'static>(
    name: &'static str,
    timeout: Duration,
    f: impl FnOnce(&mut Ctx) -> R + Send + 'static,
) -> Option<R> {
    if hung() {
        return None;
    }
    let (rtx, rrx) = mpsc::sync_channel(1);
    let job: Job = Box::new(move |ctx| {
        let _ = rtx.send(f(ctx));
    });
    sender().lock().unwrap_or_else(|e| e.into_inner()).send(job).ok()?;
    match rrx.recv_timeout(timeout) {
        Ok(r) => Some(r),
        Err(_) => {
            HUNG.store(true, Ordering::Relaxed);
            log(format!("libsm64 HUNG in `{name}` (> {} ms); Mario disabled until restart", timeout.as_millis()));
            None
        }
    }
}
