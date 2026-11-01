use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};
use vst3::ComPtr;
use vst3::Steinberg::Linux::{FileDescriptor, IEventHandler, IEventHandlerTrait, ITimerHandler, ITimerHandlerTrait, TimerInterval};

/// Registrations kept per view; a plugin needs a handful.
const MAX_ENTRIES: usize = 256;
/// Longest timer period accepted (one hour).
const MAX_INTERVAL_MS: TimerInterval = 3_600_000;

struct Timer {
    handler: ComPtr<ITimerHandler>,
    every: Duration,
    next: Instant,
}

#[derive(Default)]
struct Entries {
    fds: Vec<(ComPtr<IEventHandler>, FileDescriptor)>,
    timers: Vec<Timer>,
}

#[derive(Default)]
pub struct RunLoop {
    entries: Mutex<Entries>,
}

impl RunLoop {
    fn lock(&self) -> std::sync::MutexGuard<'_, Entries> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn add_fd(&self, handler: ComPtr<IEventHandler>, fd: FileDescriptor) -> bool {
        let mut e = self.lock();
        if fd < 0 || e.fds.len() >= MAX_ENTRIES {
            return false;
        }
        e.fds.push((handler, fd));
        log::debug!("vst3: run loop: watching fd {fd}");
        true
    }

    /// Removes every descriptor registered with this handler. The removed references are released
    /// after the lock is: a plugin destructor may call back into the run loop.
    pub fn remove_fd(&self, handler: *mut IEventHandler) -> bool {
        let gone: Vec<_> = {
            let mut e = self.lock();
            let (keep, gone) = std::mem::take(&mut e.fds).into_iter().partition(|(h, _)| h.as_ptr() != handler);
            e.fds = keep;
            gone
        };
        !gone.is_empty()
    }

    pub fn add_timer(&self, handler: ComPtr<ITimerHandler>, milliseconds: TimerInterval) -> bool {
        let mut e = self.lock();
        if e.timers.len() >= MAX_ENTRIES {
            return false;
        }
        let every = Duration::from_millis(milliseconds.clamp(1, MAX_INTERVAL_MS));
        e.timers.push(Timer { handler, every, next: Instant::now() + every });
        log::debug!("vst3: run loop: timer every {} ms", every.as_millis());
        true
    }

    /// Removes the timer registered with this handler (released after the lock, as above).
    pub fn remove_timer(&self, handler: *mut ITimerHandler) -> bool {
        let gone: Vec<_> = {
            let mut e = self.lock();
            let (keep, gone) = std::mem::take(&mut e.timers).into_iter().partition(|t| t.handler.as_ptr() != handler);
            e.timers = keep;
            gone
        };
        !gone.is_empty()
    }

    /// Drops every registration; the references are released after the lock, as above.
    pub fn clear(&self) {
        let old = std::mem::take(&mut *self.lock());
        drop(old);
    }

    fn has_fd(&self, handler: *mut IEventHandler, fd: FileDescriptor) -> bool {
        self.lock().fds.iter().any(|(h, f)| h.as_ptr() == handler && *f == fd)
    }

    fn has_timer(&self, handler: *mut ITimerHandler) -> bool {
        self.lock().timers.iter().any(|t| t.handler.as_ptr() == handler)
    }

    /// Calls the handlers whose descriptor is readable and the timers that are due (at most
    /// once each per call: a late UI frame does not replay missed ticks). Handlers may
    /// (un)register from inside their callback, so none is called with the lock held, and
    /// one removed earlier in the same pass is skipped.
    pub fn dispatch(&self, now: Instant) {
        let (fds, due) = {
            let mut e = self.lock();
            let fds = e.fds.clone();
            let mut due = Vec::new();
            for t in e.timers.iter_mut().filter(|t| t.next <= now) {
                t.next = now + t.every;
                due.push(t.handler.clone());
            }
            (fds, due)
        };
        for (h, fd) in ready(&fds) {
            if self.has_fd(h.as_ptr(), fd) {
                // SAFETY: a live handler we hold a reference to; main thread (the caller's).
                unsafe { h.onFDIsSet(fd) };
            }
        }
        for h in due {
            if self.has_timer(h.as_ptr()) {
                // SAFETY: as above.
                unsafe { h.onTimer() };
            }
        }
    }
}

/// The registrations whose descriptor has something to read (or hung up), without waiting.
fn ready(fds: &[(ComPtr<IEventHandler>, FileDescriptor)]) -> Vec<(ComPtr<IEventHandler>, FileDescriptor)> {
    if fds.is_empty() {
        return Vec::new();
    }
    let mut polled: Vec<libc::pollfd> = fds.iter().map(|&(_, fd)| libc::pollfd { fd, events: libc::POLLIN, revents: 0 }).collect();
    // SAFETY: `polled` is a valid, writable array of `polled.len()` entries; a zero timeout
    // never blocks.
    let n = unsafe { libc::poll(polled.as_mut_ptr(), polled.len() as libc::nfds_t, 0) };
    if n <= 0 {
        return Vec::new();
    }
    fds.iter()
        .zip(&polled)
        .filter(|(_, p)| p.revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0)
        .map(|((h, fd), _)| (h.clone(), *fd))
        .collect()
}
