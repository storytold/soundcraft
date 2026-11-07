use clap_sys::ext::posix_fd_support::{CLAP_POSIX_FD_ERROR, CLAP_POSIX_FD_READ, CLAP_POSIX_FD_WRITE, clap_posix_fd_flags};
use clap_sys::id::clap_id;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

/// Registrations kept per instance; a plugin needs a handful.
const MAX_ENTRIES: usize = 256;
/// Longest timer period accepted (one hour).
const MAX_PERIOD_MS: u32 = 3_600_000;

struct Timer {
    id: clap_id,
    every: Duration,
    next: Instant,
}

#[derive(Default)]
struct Entries {
    timers: Vec<Timer>,
    fds: Vec<(i32, clap_posix_fd_flags)>,
    next_id: clap_id,
}

#[derive(Default)]
pub struct RunLoop {
    entries: Mutex<Entries>,
}

impl RunLoop {
    fn lock(&self) -> std::sync::MutexGuard<'_, Entries> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn add_timer(&self, period_ms: u32) -> Option<clap_id> {
        let mut e = self.lock();
        if e.timers.len() >= MAX_ENTRIES {
            return None;
        }
        // Ids are never reused within an instance (`CLAP_INVALID_ID` is u32::MAX).
        let id = e.next_id;
        e.next_id = e.next_id.checked_add(1).filter(|&n| n != clap_id::MAX)?;
        let every = Duration::from_millis(u64::from(period_ms.clamp(1, MAX_PERIOD_MS)));
        e.timers.push(Timer { id, every, next: Instant::now() + every });
        log::debug!("clap: run loop: timer {id} every {} ms", every.as_millis());
        Some(id)
    }

    pub fn remove_timer(&self, id: clap_id) -> bool {
        let mut e = self.lock();
        let before = e.timers.len();
        e.timers.retain(|t| t.id != id);
        e.timers.len() != before
    }

    pub fn add_fd(&self, fd: i32, flags: clap_posix_fd_flags) -> bool {
        let mut e = self.lock();
        if fd < 0 || e.fds.len() >= MAX_ENTRIES || e.fds.iter().any(|&(f, _)| f == fd) {
            return false;
        }
        e.fds.push((fd, flags));
        log::debug!("clap: run loop: watching fd {fd}");
        true
    }

    pub fn modify_fd(&self, fd: i32, flags: clap_posix_fd_flags) -> bool {
        let mut e = self.lock();
        match e.fds.iter_mut().find(|(f, _)| *f == fd) {
            Some(entry) => {
                entry.1 = flags;
                true
            }
            None => false,
        }
    }

    pub fn remove_fd(&self, fd: i32) -> bool {
        let mut e = self.lock();
        let before = e.fds.len();
        e.fds.retain(|&(f, _)| f != fd);
        e.fds.len() != before
    }

    pub fn clear(&self) {
        let mut e = self.lock();
        e.timers.clear();
        e.fds.clear();
    }

    pub fn has_timer(&self, id: clap_id) -> bool {
        self.lock().timers.iter().any(|t| t.id == id)
    }

    pub fn has_fd(&self, fd: i32) -> bool {
        self.lock().fds.iter().any(|&(f, _)| f == fd)
    }

    /// The timers due at `now`, rescheduled from `now` (a late UI frame fires each once
    /// rather than replaying missed ticks).
    pub fn due(&self, now: Instant) -> Vec<clap_id> {
        let mut e = self.lock();
        let mut out = Vec::new();
        for t in e.timers.iter_mut().filter(|t| t.next <= now) {
            t.next = now + t.every;
            out.push(t.id);
        }
        out
    }

    /// The registered descriptors that are ready for what they asked for (or failed), with
    /// the matching CLAP flags, without waiting.
    pub fn ready(&self) -> Vec<(i32, clap_posix_fd_flags)> {
        let fds = self.lock().fds.clone();
        if fds.is_empty() {
            return Vec::new();
        }
        let mut polled: Vec<libc::pollfd> = fds
            .iter()
            .map(|&(fd, flags)| {
                let mut events = 0;
                if flags & CLAP_POSIX_FD_READ != 0 {
                    events |= libc::POLLIN;
                }
                if flags & CLAP_POSIX_FD_WRITE != 0 {
                    events |= libc::POLLOUT;
                }
                libc::pollfd { fd, events, revents: 0 }
            })
            .collect();
        // SAFETY: `polled` is a valid, writable array of `polled.len()` entries; a zero
        // timeout never blocks.
        if unsafe { libc::poll(polled.as_mut_ptr(), polled.len() as libc::nfds_t, 0) } <= 0 {
            return Vec::new();
        }
        polled
            .iter()
            .filter_map(|p| {
                let mut flags = 0;
                if p.revents & libc::POLLIN != 0 {
                    flags |= CLAP_POSIX_FD_READ;
                }
                if p.revents & libc::POLLOUT != 0 {
                    flags |= CLAP_POSIX_FD_WRITE;
                }
                if p.revents & (libc::POLLERR | libc::POLLHUP) != 0 {
                    flags |= CLAP_POSIX_FD_ERROR;
                }
                (flags != 0).then_some((p.fd, flags))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timers_and_descriptors_follow_registration() {
        let rl = RunLoop::default();
        // Before registering: every timer's first tick is then later than `t0`.
        let t0 = Instant::now();
        let a = rl.add_timer(10).unwrap();
        let b = rl.add_timer(0).unwrap();
        assert_ne!(a, b, "ids are unique");
        assert!(rl.due(t0).is_empty(), "not due yet");
        let mut due = rl.due(t0 + Duration::from_secs(1));
        due.sort_unstable();
        assert_eq!(due, vec![a, b], "a late frame fires each timer once");
        assert!(rl.remove_timer(a) && !rl.remove_timer(a));
        assert_eq!(rl.due(t0 + Duration::from_secs(2)), vec![b]);
        let mut fds = [0; 2];
        // SAFETY: `fds` holds the two descriptors `pipe` writes.
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        assert!(rl.add_fd(fds[0], CLAP_POSIX_FD_READ));
        assert!(!rl.add_fd(fds[0], CLAP_POSIX_FD_READ), "registered once");
        assert!(!rl.add_fd(-1, CLAP_POSIX_FD_READ));
        assert!(rl.ready().is_empty(), "nothing to read");
        // SAFETY: a valid one-byte buffer written to our own pipe.
        assert_eq!(unsafe { libc::write(fds[1], b"x".as_ptr().cast(), 1) }, 1);
        assert_eq!(rl.ready(), vec![(fds[0], CLAP_POSIX_FD_READ)]);
        assert!(rl.modify_fd(fds[0], CLAP_POSIX_FD_WRITE));
        assert!(rl.ready().is_empty(), "a read end never becomes writable");
        assert!(rl.remove_fd(fds[0]) && !rl.has_fd(fds[0]));
        rl.clear();
        assert!(!rl.has_timer(b));
        // SAFETY: closing the pipe we opened.
        unsafe {
            libc::close(fds[0]);
            libc::close(fds[1]);
        }
    }
}
