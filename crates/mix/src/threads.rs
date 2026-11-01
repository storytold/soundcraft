use super::AUDIO_THREAD;

/// Mark the calling thread as the realtime audio thread (`soundcraft_playback`'s cpal callbacks
/// do, every call: one thread-local store). Strips [`MixEngine::render`] hands to worker threads
/// count as the audio thread while they render for it. Code that might run there asks
/// [`on_audio_thread`] before doing anything that blocks; the desktop app's logger does, so a
/// `log::` record from the audio thread never takes a lock or touches a file.
pub fn mark_audio_thread() {
    AUDIO_THREAD.set(true);
}

/// True on the thread [`mark_audio_thread`] marked, and on a worker while it renders strips for
/// it. Lock-free and allocation-free.
pub fn on_audio_thread() -> bool {
    AUDIO_THREAD.get()
}

/// Run `f` on this thread as (or as not) the audio thread, then restore the thread's own mark
/// (also when `f` unwinds): a pool worker renders for realtime playback and offline bounces alike.
#[cfg(not(target_arch = "wasm32"))]
fn as_audio_thread(audio: bool, f: impl FnOnce()) {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            AUDIO_THREAD.set(self.0);
        }
    }
    let _restore = Restore(AUDIO_THREAD.replace(audio));
    f();
}

/// Process strips, in parallel on native targets when there are enough of them.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn run_strips<T: Send>(work: &mut [T], f: impl Fn(&mut T) + Sync + Send) {
    use rayon::prelude::*;
    if work.len() >= 4 {
        // The workers render on behalf of this thread: on the audio thread, they are it too.
        let audio = on_audio_thread();
        work.par_iter_mut().for_each(|w| as_audio_thread(audio, || f(w)));
    } else {
        work.iter_mut().for_each(f);
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn run_strips<T: Send>(work: &mut [T], f: impl Fn(&mut T) + Sync + Send) {
    work.iter_mut().for_each(f);
}

#[cfg(test)]
mod audio_thread_tests {
    use super::*;

    #[test]
    fn only_a_marked_thread_is_the_audio_thread() {
        assert!(!on_audio_thread());
        let marked = std::thread::spawn(|| {
            let before = on_audio_thread();
            mark_audio_thread();
            (before, on_audio_thread())
        })
        .join()
        .unwrap();
        assert_eq!(marked, (false, true));
        assert!(!on_audio_thread(), "marking another thread leaves this one alone");
    }

    /// What `on_audio_thread` says inside each strip job, for strips run from a thread that is
    /// (or is not) the audio thread.
    fn seen_by_strips(audio: bool) -> Vec<bool> {
        std::thread::spawn(move || {
            if audio {
                mark_audio_thread();
            }
            let mut work = vec![None; 64];
            run_strips(&mut work, |w| {
                // Long enough that the pool's workers take part.
                std::thread::sleep(std::time::Duration::from_millis(1));
                *w = Some(on_audio_thread());
            });
            work.into_iter().map(|w| w.unwrap()).collect()
        })
        .join()
        .unwrap()
    }

    #[test]
    fn strips_rendered_for_the_audio_thread_count_as_the_audio_thread_and_only_then() {
        assert!(seen_by_strips(true).iter().all(|&rt| rt), "a worker rendering for the audio thread is on it");
        assert!(seen_by_strips(false).iter().all(|&rt| !rt), "the workers don't stay marked afterwards");
    }
}
