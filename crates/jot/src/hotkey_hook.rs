// Copyright 2026 Google LLC
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     https://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! The global keyboard hook that arms dictation.
//!
//! `RegisterHotKey` cannot bind a bare modifier, so this is a
//! `WH_KEYBOARD_LL` hook. Two rules follow from that and are not negotiable:
//!
//! 1. The hook procedure runs on the message loop of the thread that installed
//!    it, and Windows silently *unhooks* a procedure that takes too long. So it
//!    does the smallest possible classification and hands everything else to a
//!    consumer task through a channel.
//! 2. Whether to swallow a key must be decided synchronously, inside the
//!    procedure. That is what the atomics below are for: they mirror the
//!    consumer's grammar state so the hook never has to ask anyone.

use jot_core::hotkey::{HotkeyEvent, HotkeyKey};
use jot_core::win32::SYNTHETIC_TAG;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

/// A hook event with the monotonic timestamp the grammar needs.
pub type TimedEvent = (HotkeyEvent, Duration);

static EVENTS: OnceLock<UnboundedSender<TimedEvent>> = OnceLock::new();
static EPOCH: OnceLock<Instant> = OnceLock::new();
/// The virtual-key code currently bound to dictation.
static HOTKEY_VK: AtomicU32 = AtomicU32::new(0);
/// Escape is intercepted only while the coordinator can cancel.
static CANCEL_ENABLED: AtomicBool = AtomicBool::new(false);
/// Space is intercepted only for the legacy hold-to-lock gesture.
static SPACE_LOCK_ENABLED: AtomicBool = AtomicBool::new(false);

pub fn set_hotkey(key: HotkeyKey) {
    let previous = HOTKEY_VK.swap(key.vk_code(), Ordering::SeqCst);
    if previous != key.vk_code() {
        tracing::info!(key = key.display_name(), "dictation key bound");
    }
}

pub fn set_cancel_enabled(enabled: bool) {
    CANCEL_ENABLED.store(enabled, Ordering::SeqCst);
}

pub fn set_space_lock_enabled(enabled: bool) {
    SPACE_LOCK_ENABLED.store(enabled, Ordering::SeqCst);
}

pub fn now() -> Duration {
    EPOCH.get_or_init(Instant::now).elapsed()
}

/// Installs the hook on a dedicated thread and returns the event stream.
///
/// The thread runs its own message loop for the lifetime of the process; a
/// low-level keyboard hook is only ever called on a thread that pumps messages.
pub fn install(key: HotkeyKey) -> UnboundedReceiver<TimedEvent> {
    let (tx, rx) = unbounded_channel();
    let _ = EVENTS.set(tx);
    let _ = EPOCH.set(Instant::now());
    set_hotkey(key);

    std::thread::Builder::new()
        .name("jot-hotkey".into())
        .spawn(platform::run_hook_thread)
        .expect("spawning the hotkey thread");
    rx
}

fn emit(event: HotkeyEvent) {
    if let Some(events) = EVENTS.get() {
        let _ = events.send((event, now()));
    }
}

/// What the hook decided about a single key event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Decision {
    event: Option<HotkeyEvent>,
    swallow: bool,
}

impl Decision {
    const IGNORE: Decision = Decision {
        event: None,
        swallow: false,
    };

    fn pass(event: HotkeyEvent) -> Self {
        Decision {
            event: Some(event),
            swallow: false,
        }
    }

    fn eat(event: HotkeyEvent) -> Self {
        Decision {
            event: Some(event),
            swallow: true,
        }
    }
}

const VK_ESCAPE: u32 = 0x1B;
const VK_SPACE: u32 = 0x20;

/// Bare modifiers are not "another key was typed": holding Shift to capitalise
/// mid-dictation must not read as an accidental chord.
fn is_modifier(vk: u32) -> bool {
    matches!(
        vk,
        0x10 | 0x11 | 0x12 // VK_SHIFT / VK_CONTROL / VK_MENU
            | 0xA0
            ..=0xA5   // sided shift, control, alt
            | 0x5B | 0x5C   // VK_LWIN / VK_RWIN
            | 0x14 // VK_CAPITAL
    )
}

/// The hook policy, testable without a message loop.
fn classify(
    vk: u32,
    is_down: bool,
    hotkey: HotkeyKey,
    pressed_key: &mut Option<HotkeyKey>,
    cancel_enabled: bool,
    space_lock_enabled: bool,
) -> Decision {
    // A rebind can race with a queued key-down. Keep both halves of the
    // physical press on the same binding, including its suppression policy.
    let key = pressed_key.unwrap_or(hotkey);
    if vk == key.vk_code() {
        *pressed_key = is_down.then_some(key);
        let event = if is_down {
            HotkeyEvent::HotkeyDown
        } else {
            HotkeyEvent::HotkeyUp
        };
        return if key.must_suppress() {
            // Caps Lock toggles a global state and the Windows key opens the
            // Start menu on release. Either would fire on every dictation.
            Decision::eat(event)
        } else {
            Decision::pass(event)
        };
    }
    if !is_down {
        return Decision::IGNORE;
    }
    if vk == VK_ESCAPE {
        return if cancel_enabled {
            Decision::eat(HotkeyEvent::EscDown)
        } else {
            // Report even before the consumer has handled a queued Begin, but
            // let Windows see Esc whenever Jot isn't known to be cancellable.
            Decision::pass(HotkeyEvent::EscDown)
        };
    }
    if vk == VK_SPACE && space_lock_enabled && pressed_key.is_some() {
        // The hands-free gesture. Swallowed so it does not also type a space
        // into whatever the user is dictating into.
        return Decision::eat(HotkeyEvent::SpaceLock);
    }
    if is_modifier(vk) {
        return Decision::IGNORE;
    }
    Decision::pass(HotkeyEvent::OtherKeyDown)
}

#[cfg(target_os = "windows")]
mod platform {
    use super::*;
    use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, DispatchMessageW, GetMessageW, HC_ACTION, KBDLLHOOKSTRUCT, MSG,
        SetWindowsHookExW, TranslateMessage, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN,
        WM_SYSKEYUP,
    };

    thread_local! {
        static PRESSED_KEY: std::cell::RefCell<Option<HotkeyKey>> = const {
            std::cell::RefCell::new(None)
        };
    }

    pub fn run_hook_thread() {
        let hook = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), None, 0) };
        let Ok(_hook) = hook else {
            tracing::error!("could not install the keyboard hook — dictation key is inert");
            return;
        };
        // A low-level keyboard hook is dispatched through this thread's message
        // queue, so the loop is what makes the hook fire at all.
        let mut message = MSG::default();
        unsafe {
            while GetMessageW(&mut message, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }

    unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code != HC_ACTION as i32 {
            return unsafe { CallNextHookEx(None, code, wparam, lparam) };
        }
        let info = unsafe { *(lparam.0 as *const KBDLLHOOKSTRUCT) };

        // Ignore only OUR OWN synthetic events: the Ctrl+V from the insertion
        // ladder would otherwise read as an accidental chord and cancel the next
        // dictation. Injected input in general is NOT ignored — that would make
        // Jot inert over Remote Desktop, on the on-screen keyboard, and under
        // any remapping tool, all of which deliver the user's real keystrokes
        // with `LLKHF_INJECTED` set.
        if info.dwExtraInfo == SYNTHETIC_TAG {
            return unsafe { CallNextHookEx(None, code, wparam, lparam) };
        }

        let message = wparam.0 as u32;
        let is_down = message == WM_KEYDOWN || message == WM_SYSKEYDOWN;
        let is_up = message == WM_KEYUP || message == WM_SYSKEYUP;
        if !is_down && !is_up {
            return unsafe { CallNextHookEx(None, code, wparam, lparam) };
        }

        let decision = PRESSED_KEY.with(|pressed_key| {
            classify(
                info.vkCode,
                is_down,
                HotkeyKey::from_vk(HOTKEY_VK.load(Ordering::SeqCst)).unwrap_or_default(),
                &mut pressed_key.borrow_mut(),
                CANCEL_ENABLED.load(Ordering::SeqCst),
                SPACE_LOCK_ENABLED.load(Ordering::SeqCst),
            )
        });
        if let Some(event) = decision.event {
            emit(event);
        }
        if decision.swallow {
            return LRESULT(1);
        }
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    }
}

#[cfg(not(target_os = "windows"))]
mod platform {
    pub fn run_hook_thread() {}
}

#[cfg(test)]
mod tests {
    use super::*;

    use HotkeyKey::{CapsLock, RightControl};

    const RIGHT_CTRL: u32 = 0xA3;
    const CAPS_LOCK: u32 = 0x14;
    const KEY_A: u32 = 0x41;

    #[test]
    fn the_hotkey_is_reported_and_passed_through_when_it_carries_no_state() {
        let mut held = None;
        assert_eq!(
            classify(RIGHT_CTRL, true, RightControl, &mut held, false, false),
            Decision::pass(HotkeyEvent::HotkeyDown)
        );
        assert_eq!(
            classify(RIGHT_CTRL, false, RightControl, &mut held, false, false),
            Decision::pass(HotkeyEvent::HotkeyUp)
        );
    }

    #[test]
    fn caps_lock_is_swallowed_so_it_never_toggles_while_dictating() {
        assert_eq!(
            classify(CAPS_LOCK, true, CapsLock, &mut None, false, false),
            Decision::eat(HotkeyEvent::HotkeyDown)
        );
    }

    #[test]
    fn changing_the_binding_still_reports_the_old_release() {
        let mut held = None;
        assert_eq!(
            classify(CAPS_LOCK, true, CapsLock, &mut held, false, false),
            Decision::eat(HotkeyEvent::HotkeyDown)
        );
        assert_eq!(
            classify(CAPS_LOCK, false, RightControl, &mut held, false, false),
            Decision::eat(HotkeyEvent::HotkeyUp)
        );
        assert_eq!(
            classify(RIGHT_CTRL, true, RightControl, &mut held, false, false),
            Decision::pass(HotkeyEvent::HotkeyDown)
        );
    }

    #[test]
    fn escape_is_only_intercepted_while_a_session_is_live() {
        assert_eq!(
            classify(VK_ESCAPE, true, RightControl, &mut None, false, false),
            Decision::pass(HotkeyEvent::EscDown)
        );
        assert_eq!(
            classify(VK_ESCAPE, true, RightControl, &mut None, true, false),
            Decision::eat(HotkeyEvent::EscDown)
        );
    }

    #[test]
    fn space_is_only_intercepted_for_the_hold_mode_gesture() {
        assert_eq!(
            classify(VK_SPACE, true, RightControl, &mut None, true, true),
            Decision::pass(HotkeyEvent::OtherKeyDown)
        );
        assert_eq!(
            classify(
                VK_SPACE,
                true,
                RightControl,
                &mut Some(RightControl),
                true,
                false
            ),
            Decision::pass(HotkeyEvent::OtherKeyDown)
        );
        assert_eq!(
            classify(
                VK_SPACE,
                true,
                RightControl,
                &mut Some(RightControl),
                true,
                true
            ),
            Decision::eat(HotkeyEvent::SpaceLock)
        );
    }

    #[test]
    fn a_bare_modifier_is_not_an_accidental_chord() {
        assert_eq!(
            classify(0x10, true, RightControl, &mut None, false, false),
            Decision::IGNORE
        );
        assert_eq!(
            classify(0xA0, true, RightControl, &mut None, false, false),
            Decision::IGNORE
        );
        assert_eq!(
            classify(KEY_A, true, RightControl, &mut None, false, false),
            Decision::pass(HotkeyEvent::OtherKeyDown)
        );
    }

    #[test]
    fn key_releases_other_than_the_hotkey_are_not_events() {
        assert_eq!(
            classify(KEY_A, false, RightControl, &mut None, false, false),
            Decision::IGNORE
        );
        assert_eq!(
            classify(VK_SPACE, false, RightControl, &mut None, false, false),
            Decision::IGNORE
        );
    }

    #[test]
    fn nothing_the_hook_swallows_is_ever_left_unreported() {
        for vk in [CAPS_LOCK, VK_ESCAPE, VK_SPACE] {
            let decision = classify(vk, true, CapsLock, &mut Some(CapsLock), true, true);
            if decision.swallow {
                assert!(
                    decision.event.is_some(),
                    "a swallowed key that reports nothing is a keystroke the user lost"
                );
            }
        }
    }
}
