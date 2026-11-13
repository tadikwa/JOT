//! Lets this copy's own setup ask it to close before replacing it.
//!
//! The setup signals a named event derived from the install folder
//! ([`jot_core::update::quit_event_name`]); Jot then quits the normal way, and
//! only when nothing is in progress. Nothing is ever killed: a busy Jot stays
//! open and the setup tells the user so.

use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{CreateEventW, INFINITE, WaitForSingleObject};
use windows::core::HSTRING;

/// One message per request. None if the event could not be created (the setup
/// then falls back to asking the user to close Jot).
pub fn listen() -> Option<UnboundedReceiver<()>> {
    let exe = std::env::current_exe().ok()?;
    let name = HSTRING::from(jot_core::update::quit_event_name(exe.parent()?));
    // Auto-reset: each SetEvent wakes the wait once. Same-session namespace,
    // default security: only this user's processes can open it.
    let event = match unsafe { CreateEventW(None, false, false, &name) } {
        Ok(event) => event,
        Err(error) => {
            tracing::warn!(%error, "no quit-request event; the setup will ask to close Jot by hand");
            return None;
        }
    };
    // The handle lives as long as the process; HANDLE is not Send, its value is.
    let raw = event.0 as usize;
    let (tx, rx) = unbounded_channel();
    std::thread::Builder::new()
        .name("jot-quit-request".into())
        .spawn(move || {
            let event = HANDLE(raw as _);
            while unsafe { WaitForSingleObject(event, INFINITE) } == WAIT_OBJECT_0 {
                if tx.send(()).is_err() {
                    break;
                }
            }
        })
        .ok()?;
    Some(rx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{EVENT_MODIFY_STATE, OpenEventW, SetEvent};

    /// What the setup does (OpenEventW + SetEvent on the folder's name) reaches
    /// the listener once per signal.
    #[test]
    fn a_request_from_the_setup_arrives_once_per_signal() {
        let mut requests = listen().expect("listener");
        let exe = std::env::current_exe().unwrap();
        let name = HSTRING::from(jot_core::update::quit_event_name(exe.parent().unwrap()));
        let event = unsafe { OpenEventW(EVENT_MODIFY_STATE, false, &name) }.unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let next = |requests: &mut UnboundedReceiver<()>, wait| {
            runtime.block_on(async {
                tokio::time::timeout(std::time::Duration::from_millis(wait), requests.recv()).await
            })
        };
        assert!(next(&mut requests, 200).is_err(), "nothing before a signal");
        for _ in 0..2 {
            unsafe { SetEvent(event) }.unwrap();
            assert_eq!(next(&mut requests, 2000), Ok(Some(())));
        }
        unsafe { CloseHandle(event) }.unwrap();
    }
}
