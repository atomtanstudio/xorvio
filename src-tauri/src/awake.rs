//! Keeps the computer from idle-sleeping while something records: with
//! caffeinate on macOS, systemd-inhibit on Linux and the thread execution
//! state on Windows. Where none of them is there, recordings go on as
//! before and the computer sleeps when it would have.

use std::sync::Mutex;

#[derive(Default)]
pub(crate) struct KeepAwake(Mutex<Option<Hold>>);

impl KeepAwake {
    pub fn set(&self, on: bool) {
        let mut hold = self.0.lock().unwrap();
        if on && hold.is_none() {
            *hold = Hold::take();
        } else if !on {
            // Letting go of the hold lets the computer sleep again.
            *hold = None;
        }
    }
}

/// A program that keeps the computer awake for as long as it runs.
#[cfg(any(target_os = "macos", target_os = "linux"))]
struct Hold(std::process::Child);

#[cfg(target_os = "macos")]
impl Hold {
    fn take() -> Option<Hold> {
        // No idle sleep, and only while Xorvio runs, should it quit without letting go.
        std::process::Command::new("/usr/bin/caffeinate")
            .args(["-i", "-w", &std::process::id().to_string()])
            .spawn()
            .ok()
            .map(Hold)
    }
}

#[cfg(target_os = "linux")]
impl Hold {
    fn take() -> Option<Hold> {
        use std::process::Stdio;
        // No idle sleep or suspend while `cat` runs. It reads a pipe from
        // Xorvio, so should Xorvio quit without letting go, the pipe closes,
        // `cat` ends and the hold goes with it.
        std::process::Command::new("systemd-inhibit")
            .args(["--what=idle:sleep", "--who=Xorvio", "--why=Recording", "--mode=block", "cat"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .inspect_err(|error| tracing::warn!("systemd-inhibit couldn't be started, so the computer may sleep while recording: {error}"))
            .ok()
            .map(Hold)
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
impl Drop for Hold {
    fn drop(&mut self) {
        drop(self.0.stdin.take());
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A thread whose execution state keeps the computer awake until it's told
/// to let go; Windows keeps the state only while the thread that set it lives.
#[cfg(windows)]
struct Hold {
    release: std::sync::mpsc::Sender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
}

#[cfg(windows)]
impl Hold {
    fn take() -> Option<Hold> {
        use windows_sys::Win32::System::Power::{ES_CONTINUOUS, ES_SYSTEM_REQUIRED, SetThreadExecutionState};
        let (release, released) = std::sync::mpsc::channel::<()>();
        let thread = std::thread::Builder::new()
            .name("keep-awake".into())
            .spawn(move || {
                // SAFETY: it takes and returns plain flags.
                unsafe { SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED) };
                let _ = released.recv();
                unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
            })
            .inspect_err(|error| tracing::warn!("the computer may sleep while recording: {error}"))
            .ok()?;
        Some(Hold { release, thread: Some(thread) })
    }
}

#[cfg(windows)]
impl Drop for Hold {
    fn drop(&mut self) {
        let _ = self.release.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
struct Hold;

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
impl Hold {
    fn take() -> Option<Hold> {
        None
    }
}
