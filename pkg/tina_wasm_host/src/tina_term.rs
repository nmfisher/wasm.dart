//! The `tina:term` host import: the tty size authority (`get-size` answers
//! with the live `TIOCGWINSZ` window — the guest polls, WASI has no
//! guest-side signals) and the crash channel (spike finding 4): a guest
//! Dart exception calls `fail` from its top-level zone guard BEFORE the
//! WASI `exit` backstop, so the TUI runner's parked `run` wait can select
//! on the signal, restore the terminal, and report
//! `guest crashed: <message>` with a non-zero exit instead of parking
//! forever.
//!
//! Both interfaces are flat functions (no resources, no results), so they
//! link with plain `func_wrap` — the same way the runner links
//! `wasmdart:tests/result-collector` and the plugin session. No bindgen.
//! The guest-side shapes live in the tina repo
//! (`tina_wasm/wit/deps/tina-term/package.wit`) and must stay in sync.

use std::sync::{Arc, Mutex};

use tokio::sync::Notify;
use wasmtime::component::Linker;

/// The live window size (`TIOCGWINSZ` on stdin), the host's tty authority.
pub fn window_size() -> Result<(u32, u32), wasmtime::Error> {
    use std::os::fd::AsRawFd as _;
    let mut winsize = libc::winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // Safety: a plain TIOCGWINSZ ioctl on the process's own stdin; a
    // non-tty stdin fails with ENOTTY and the guest falls back to its
    // default layout.
    let rc =
        unsafe { libc::ioctl(std::io::stdin().as_raw_fd(), libc::TIOCGWINSZ, &mut winsize) };
    if rc != 0 || winsize.ws_col == 0 || winsize.ws_row == 0 {
        return Ok((80, 24));
    }
    Ok((winsize.ws_col as u32, winsize.ws_row as u32))
}

/// The crash channel's shared state: the recorded message plus the notify
/// the TUI runner's parked wait selects on.
#[derive(Clone, Default)]
pub struct CrashState {
    inner: Arc<CrashInner>,
}

#[derive(Default)]
struct CrashInner {
    message: Mutex<Option<String>>,
    notify: Notify,
}

impl CrashState {
    /// The guest called `fail`: record and wake the waiter (if any). The
    /// WASI `exit` backstop the guest invokes right after ends the
    /// instance; this state outlives the call.
    pub fn fail(&self, message: String) {
        let mut slot = self.inner.message.lock().unwrap();
        if slot.is_none() {
            *slot = Some(message);
        }
        self.inner.notify.notify_waiters();
    }

    /// The recorded message, if the guest already crashed (used by the
    /// instantiate-failure path, which runs before the parked wait).
    pub fn take(&self) -> Option<String> {
        self.inner.message.lock().unwrap().take()
    }

    /// Resolve with the crash message: immediately when `fail` already
    /// ran, otherwise when it does.
    pub async fn wait(&self) -> String {
        loop {
            if let Some(message) = self.inner.message.lock().unwrap().clone() {
                return message;
            }
            self.inner.notify.notified().await;
        }
    }
}

/// Adds the `tina:term/tty@0.1.0` + `tina:term/crash@0.1.0` instances to
/// `linker`. Components that do not import `tina:term` never reach it.
pub fn add_to_linker(
    linker: &mut Linker<crate::HostState>,
) -> wasmtime::Result<()> {
    {
        let mut root = linker.root();
        let mut tty = root.instance("tina:term/tty@0.1.0")?;
        tty.func_wrap("get-size", |_store, _params: ()| -> wasmtime::Result<((u32, u32),)> {
            Ok((window_size()?,))
        })?;
    }
    {
        let mut root = linker.root();
        let mut crash = root.instance("tina:term/crash@0.1.0")?;
        crash.func_wrap(
            "fail",
            |store: wasmtime::StoreContextMut<'_, crate::HostState>,
             params: (String,)|
             -> wasmtime::Result<()> {
                store.data().crash.fail(params.0);
                Ok(())
            },
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::CrashState;

    #[tokio::test]
    async fn fail_before_wait_resolves_immediately() {
        let crash = CrashState::default();
        crash.fail("boom".to_string());
        assert_eq!(crash.wait().await, "boom");
    }

    #[tokio::test]
    async fn wait_wakes_on_fail() {
        let crash = CrashState::default();
        let waiter = crash.clone();
        let handle = tokio::spawn(async move { waiter.wait().await });
        // Give the waiter a chance to park before failing.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        crash.fail("late boom".to_string());
        assert_eq!(handle.await.unwrap(), "late boom");
    }

    #[tokio::test]
    async fn first_message_wins() {
        let crash = CrashState::default();
        crash.fail("first".to_string());
        crash.fail("second".to_string());
        assert_eq!(crash.wait().await, "first");
    }
}
