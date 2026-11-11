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
use jot_core::hotkey::{HotkeyBinding, HotkeyChord, HotkeyEvent, HotkeyKey};
use jot_core::win32::SYNTHETIC_TAG;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
/// A hook event with the monotonic timestamp the grammar needs.
pub type TimedEvent = (HotkeyEvent, Duration);
static EVENTS: OnceLock<UnboundedSender<TimedEvent>> = OnceLock::new();
static EPOCH: OnceLock<Instant> = OnceLock::new();
/// The virtual-key code currently bound to dictation.
static HOTKEY_BINDING: AtomicU64 = AtomicU64::new(0);
static REQUESTED_BINDING: AtomicU64 = AtomicU64::new(0);
static BIND_ERROR: AtomicU32 = AtomicU32::new(0);
static CAPTURING: AtomicBool = AtomicBool::new(false);
static CAPTURED: AtomicU64 = AtomicU64::new(0);
static CAPTURE_READY: AtomicBool = AtomicBool::new(false);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindError {
    Invalid,
    Conflict,
    Unavailable,
}
pub fn binding_error() -> Option<BindError> {
    match BIND_ERROR.load(Ordering::SeqCst) {
        1 => Some(BindError::Invalid),
        2 => Some(BindError::Conflict),
        3 => Some(BindError::Unavailable),
        _ => None,
    }
}
pub fn prepare_binding(binding: HotkeyBinding) -> Result<(), BindError> {
    platform::rebind(binding, false)?;
    // A successful retry of the same saved shortcut must reach the apply command.
    REQUESTED_BINDING.store(0, Ordering::SeqCst);
    Ok(())
}
pub fn start_capture() {
    CAPTURED.store(0, Ordering::SeqCst);
    CAPTURE_READY.store(false, Ordering::SeqCst);
    CAPTURING.store(true, Ordering::SeqCst);
}
pub fn stop_capture() {
    CAPTURING.store(false, Ordering::SeqCst);
}
/// None = waiting, Err = Escape cancelled, Ok = physical candidate (not yet accepted).
pub fn captured_binding() -> Option<Result<HotkeyBinding, ()>> {
    match CAPTURED.load(Ordering::SeqCst) {
        0 => None,
        u64::MAX => Some(Err(())),
        value => HotkeyBinding::decode(value).map(Ok),
    }
}
/// Escape is intercepted only while the coordinator can cancel.
static CANCEL_ENABLED: AtomicBool = AtomicBool::new(false);
/// Space is intercepted only for the legacy hold-to-lock gesture.
static SPACE_LOCK_ENABLED: AtomicBool = AtomicBool::new(false);
pub fn set_hotkey(binding: HotkeyBinding) {
    if REQUESTED_BINDING.swap(binding.encode(), Ordering::SeqCst) == binding.encode() {
        return;
    }
    let error = platform::rebind(binding, true).err();
    BIND_ERROR.store(
        match error {
            None => 0,
            Some(BindError::Invalid) => 1,
            Some(BindError::Conflict) => 2,
            Some(BindError::Unavailable) => 3,
        },
        Ordering::SeqCst,
    );
    if error.is_some() {
        tracing::warn!("dictation shortcut could not be activated");
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
pub fn install(binding: HotkeyBinding) -> UnboundedReceiver<TimedEvent> {
    let (tx, rx) = unbounded_channel();
    let _ = EVENTS.set(tx);
    let _ = EPOCH.set(Instant::now());
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("jot-hotkey".into())
        .spawn(move || platform::run_hook_thread(ready_tx))
        .expect("spawning the hotkey thread");
    if ready_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap_or(false)
    {
        set_hotkey(binding);
    } else {
        BIND_ERROR.store(3, Ordering::SeqCst);
    }
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
/// Physical edges are the single event source, even for registered chords.
struct PhysicalKeyboard {
    down: [bool; 256],
    bare_pressed: Option<HotkeyKey>,
    chord_pressed: Option<HotkeyChord>,
    owned_main: Option<u32>,
}
impl Default for PhysicalKeyboard {
    fn default() -> Self {
        Self {
            down: [false; 256],
            bare_pressed: None,
            chord_pressed: None,
            owned_main: None,
        }
    }
}
impl PhysicalKeyboard {
    fn modifiers(&self) -> u32 {
        let any = |keys: &[usize]| keys.iter().any(|key| self.down[*key]);
        (u32::from(any(&[0x12, 0xA4, 0xA5])) * HotkeyChord::ALT)
            | (u32::from(any(&[0x11, 0xA2, 0xA3])) * HotkeyChord::CTRL)
            | (u32::from(any(&[0x10, 0xA0, 0xA1])) * HotkeyChord::SHIFT)
            | (u32::from(any(&[0x5B, 0x5C])) * HotkeyChord::WIN)
    }
    fn handle(
        &mut self,
        vk: u32,
        is_down: bool,
        binding: Option<HotkeyBinding>,
        cancel: bool,
        space: bool,
    ) -> Decision {
        let Some(key) = self.down.get_mut(vk as usize) else {
            return Decision::IGNORE;
        };
        let repeat = *key && is_down;
        *key = is_down;
        if CAPTURING.load(Ordering::SeqCst) {
            if CAPTURE_READY.load(Ordering::SeqCst) && vk != VK_ESCAPE {
                // Let Tab/Enter navigate the confirmation controls, without arming dictation.
                return Decision::IGNORE;
            }
            if vk == VK_ESCAPE && is_down {
                CAPTURED.store(u64::MAX, Ordering::SeqCst);
            } else if CAPTURED.load(Ordering::SeqCst) == 0 {
                if is_down && !repeat && !is_modifier(vk) {
                    CAPTURED.store(
                        HotkeyBinding::Chord(HotkeyChord {
                            vk,
                            modifiers: self.modifiers(),
                        })
                        .encode(),
                        Ordering::SeqCst,
                    );
                } else if !is_down && let Some(key) = HotkeyKey::from_vk(vk) {
                    CAPTURED.store(HotkeyBinding::Bare(key).encode(), Ordering::SeqCst);
                }
            }
            if CAPTURED.load(Ordering::SeqCst) != 0 && self.down.iter().all(|down| !down) {
                CAPTURE_READY.store(true, Ordering::SeqCst);
            }
            return Decision {
                event: None,
                swallow: true,
            };
        }
        if self.chord_pressed.is_none()
            && self.owned_main.is_none()
            && let Some(HotkeyBinding::Bare(key)) = binding
        {
            return classify(vk, is_down, key, &mut self.bare_pressed, cancel, space);
        }
        let owned = self.owned_main == Some(vk);
        if owned && !is_down {
            self.owned_main = None;
        }
        if let Some(chord) = self.chord_pressed {
            if !is_down && (vk == chord.vk || self.modifiers() & chord.modifiers != chord.modifiers)
            {
                self.chord_pressed = None;
                return Decision {
                    event: Some(HotkeyEvent::HotkeyUp),
                    swallow: owned,
                };
            }
        } else if !repeat
            && is_down
            && self.owned_main.is_none()
            && let Some(HotkeyBinding::Chord(chord)) = binding
            && vk == chord.vk
            && self.modifiers() == chord.modifiers
        {
            self.chord_pressed = Some(chord);
            self.owned_main = Some(vk);
            // Windows must see this edge to consume its registered shortcut (notably Alt menus).
            return Decision::pass(HotkeyEvent::HotkeyDown);
        }
        if owned {
            return Decision {
                event: None,
                swallow: true,
            };
        }
        if is_down && vk == VK_ESCAPE {
            return if cancel {
                Decision::eat(HotkeyEvent::EscDown)
            } else {
                Decision::pass(HotkeyEvent::EscDown)
            };
        }
        Decision::IGNORE
    }
}
#[cfg(target_os = "windows")]
mod platform {
    use super::*;
    use std::sync::mpsc::{Sender, SyncSender};
    use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        HOT_KEY_MODIFIERS, RegisterHotKey, UnregisterHotKey,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, DispatchMessageW, GetMessageW, HC_ACTION, KBDLLHOOKSTRUCT, MSG,
        PM_NOREMOVE, PeekMessageW, PostThreadMessageW, SetWindowsHookExW, TranslateMessage,
        UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_APP, WM_HOTKEY, WM_KEYDOWN, WM_KEYUP,
        WM_SYSKEYDOWN, WM_SYSKEYUP,
    };
    struct Rebind {
        binding: HotkeyBinding,
        apply: bool,
        reply: SyncSender<Result<(), BindError>>,
    }
    static COMMANDS: OnceLock<Sender<Rebind>> = OnceLock::new();
    static THREAD_ID: AtomicU32 = AtomicU32::new(0);
    const REBIND: u32 = WM_APP + 141;
    thread_local! {
        static KEYBOARD: std::cell::RefCell<PhysicalKeyboard> = std::cell::RefCell::new(PhysicalKeyboard::default());
    }
    #[derive(Default)]
    struct Registrations {
        active: Option<(HotkeyBinding, Option<i32>)>,
        pending: Option<(HotkeyBinding, Option<i32>)>,
    }
    impl Registrations {
        fn prepare(&mut self, binding: HotkeyBinding) -> Result<(), BindError> {
            if let HotkeyBinding::Chord(chord) = binding {
                chord.validate().map_err(|_| BindError::Invalid)?;
            }
            if self.active.is_some_and(|entry| entry.0 == binding) {
                if let Some((_, Some(id))) = self.pending.take() {
                    unsafe {
                        let _ = UnregisterHotKey(None, id);
                    }
                }
                return Ok(());
            }
            if self.pending.is_some_and(|entry| entry.0 == binding) {
                return Ok(());
            }
            if let Some((_, Some(id))) = self.pending.take() {
                unsafe {
                    let _ = UnregisterHotKey(None, id);
                }
            }
            let id = if let HotkeyBinding::Chord(chord) = binding {
                let id = if self.active.and_then(|entry| entry.1) == Some(0x4A01) {
                    0x4A02
                } else {
                    0x4A01
                };
                unsafe {
                    RegisterHotKey(
                        None,
                        id,
                        HOT_KEY_MODIFIERS(chord.modifiers | 0x4000),
                        chord.vk,
                    )
                }
                .map_err(|_| BindError::Conflict)?;
                Some(id)
            } else {
                None
            };
            self.pending = Some((binding, id));
            Ok(())
        }
        fn apply(&mut self, binding: HotkeyBinding) -> Result<(), BindError> {
            if self.active.is_some_and(|entry| entry.0 == binding) {
                return Ok(());
            }
            self.prepare(binding)?;
            if let Some((_, Some(id))) = self.active.take() {
                unsafe {
                    let _ = UnregisterHotKey(None, id);
                }
            }
            self.active = self.pending.take();
            HOTKEY_BINDING.store(binding.encode(), Ordering::SeqCst);
            Ok(())
        }
    }
    impl Drop for Registrations {
        fn drop(&mut self) {
            for (_, id) in self.active.take().into_iter().chain(self.pending.take()) {
                if let Some(id) = id {
                    unsafe {
                        let _ = UnregisterHotKey(None, id);
                    }
                }
            }
        }
    }

    pub fn rebind(binding: HotkeyBinding, apply: bool) -> Result<(), BindError> {
        let (reply, receive) = std::sync::mpsc::sync_channel(1);
        COMMANDS
            .get()
            .ok_or(BindError::Unavailable)?
            .send(Rebind {
                binding,
                apply,
                reply,
            })
            .map_err(|_| BindError::Unavailable)?;
        unsafe {
            PostThreadMessageW(
                THREAD_ID.load(Ordering::SeqCst),
                REBIND,
                WPARAM(0),
                LPARAM(0),
            )
        }
        .map_err(|_| BindError::Unavailable)?;
        receive
            .recv_timeout(Duration::from_secs(2))
            .unwrap_or(Err(BindError::Unavailable))
    }
    pub fn run_hook_thread(ready: SyncSender<bool>) {
        let mut message = MSG::default();
        unsafe {
            let _ = PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE);
        }
        THREAD_ID.store(unsafe { GetCurrentThreadId() }, Ordering::SeqCst);
        let (commands, receive) = std::sync::mpsc::channel();
        let _ = COMMANDS.set(commands);
        let hook = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), None, 0) };
        let Ok(_hook) = hook else {
            tracing::error!("could not install the keyboard hook — dictation key is inert");
            let _ = ready.send(false);
            return;
        };
        let mut registrations = Registrations::default();
        let _ = ready.send(true);
        unsafe {
            while GetMessageW(&mut message, None, 0, 0).as_bool() {
                if message.message == REBIND {
                    while let Ok(command) = receive.try_recv() {
                        let result = if command.apply {
                            registrations.apply(command.binding)
                        } else {
                            registrations.prepare(command.binding)
                        };
                        let _ = command.reply.send(result);
                    }
                } else if message.message != WM_HOTKEY {
                    // RegisterHotKey reserves a chord; the physical hook alone starts/stops it.
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
            let _ = UnhookWindowsHookEx(_hook);
        }
        HOTKEY_BINDING.store(0, Ordering::SeqCst);
        THREAD_ID.store(0, Ordering::SeqCst);
        BIND_ERROR.store(3, Ordering::SeqCst);
    }
    unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code != HC_ACTION as i32 {
            return unsafe { CallNextHookEx(None, code, wparam, lparam) };
        }
        let info = unsafe { *(lparam.0 as *const KBDLLHOOKSTRUCT) };
        if info.dwExtraInfo == SYNTHETIC_TAG {
            return unsafe { CallNextHookEx(None, code, wparam, lparam) };
        }
        let message = wparam.0 as u32;
        let is_down = message == WM_KEYDOWN || message == WM_SYSKEYDOWN;
        if !is_down && message != WM_KEYUP && message != WM_SYSKEYUP {
            return unsafe { CallNextHookEx(None, code, wparam, lparam) };
        }
        let decision = KEYBOARD.with(|keyboard| {
            keyboard.borrow_mut().handle(
                info.vkCode,
                is_down,
                HotkeyBinding::decode(HOTKEY_BINDING.load(Ordering::SeqCst)),
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
    #[cfg(test)]
    mod reservation_tests {
        use super::*;
        static NATIVE_HOTKEY_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        #[test]
        fn retrying_same_binding_after_startup_conflict_really_activates_it() {
            let _guard = NATIVE_HOTKEY_LOCK.lock().unwrap();
            let chord = HotkeyChord {
                modifiers: 7,
                vk: 0x85,
            };
            let binding = HotkeyBinding::Chord(chord);
            let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
            let (stop_tx, stop_rx) = std::sync::mpsc::sync_channel(1);
            let owner = std::thread::spawn(move || {
                let result = unsafe {
                    RegisterHotKey(
                        None,
                        0x4A21,
                        HOT_KEY_MODIFIERS(chord.modifiers | 0x4000),
                        chord.vk,
                    )
                };
                ready_tx.send(result.is_ok()).unwrap();
                stop_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                if result.is_ok() {
                    unsafe {
                        UnregisterHotKey(None, 0x4A21).unwrap();
                    }
                }
            });
            assert!(ready_rx.recv_timeout(Duration::from_secs(2)).unwrap());
            let _events = install(binding);
            assert_eq!(binding_error(), Some(BindError::Conflict));
            stop_tx.send(()).unwrap();
            owner.join().unwrap();
            prepare_binding(binding).unwrap();
            assert_ne!(
                HOTKEY_BINDING.load(Ordering::SeqCst),
                binding.encode(),
                "preparation alone must not activate"
            );
            set_hotkey(binding);
            let activated = HOTKEY_BINDING.load(Ordering::SeqCst) == binding.encode()
                && binding_error().is_none();
            unsafe {
                PostThreadMessageW(
                    THREAD_ID.load(Ordering::SeqCst),
                    windows::Win32::UI::WindowsAndMessaging::WM_QUIT,
                    WPARAM(0),
                    LPARAM(0),
                )
                .unwrap();
            }
            for _ in 0..100 {
                if THREAD_ID.load(Ordering::SeqCst) == 0 {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            assert_eq!(
                THREAD_ID.load(Ordering::SeqCst),
                0,
                "test keyboard hook must stop"
            );
            assert!(
                activated,
                "same shortcut must become active after its conflict is cleared"
            );
        }

        #[test]
        fn windows_collision_keeps_current_binding_and_releases_reservations() {
            let _guard = NATIVE_HOTKEY_LOCK.lock().unwrap();
            let chord = HotkeyChord {
                modifiers: 7,
                vk: 0x86,
            };
            let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
            let (stop_tx, stop_rx) = std::sync::mpsc::sync_channel(1);
            let owner = std::thread::spawn(move || {
                let result = unsafe {
                    RegisterHotKey(
                        None,
                        0x4A11,
                        HOT_KEY_MODIFIERS(chord.modifiers | 0x4000),
                        chord.vk,
                    )
                };
                ready_tx.send(result.is_ok()).unwrap();
                stop_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                if result.is_ok() {
                    unsafe {
                        UnregisterHotKey(None, 0x4A11).unwrap();
                    }
                }
            });
            assert!(
                ready_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
                "test-only Ctrl+Alt+Shift+F23 already reserved"
            );
            let mut reservations = Registrations::default();
            let bare = HotkeyBinding::Bare(HotkeyKey::RightControl);
            reservations.apply(bare).unwrap();
            assert_eq!(
                reservations.prepare(HotkeyBinding::Chord(chord)),
                Err(BindError::Conflict)
            );
            assert_eq!(reservations.active.unwrap().0, bare);
            stop_tx.send(()).unwrap();
            owner.join().unwrap();
            reservations.prepare(HotkeyBinding::Chord(chord)).unwrap();
            assert_eq!(
                reservations.active.unwrap().0,
                bare,
                "prepare must not rebind a live gesture"
            );
            reservations.apply(HotkeyBinding::Chord(chord)).unwrap();
            assert_eq!(reservations.active.unwrap().0, HotkeyBinding::Chord(chord));
            drop(reservations);
            unsafe {
                RegisterHotKey(
                    None,
                    0x4A12,
                    HOT_KEY_MODIFIERS(chord.modifiers | 0x4000),
                    chord.vk,
                )
                .unwrap();
                UnregisterHotKey(None, 0x4A12).unwrap();
            }
        }
    }
}
#[cfg(not(target_os = "windows"))]
mod platform {
    use super::*;
    pub fn run_hook_thread(ready: std::sync::mpsc::SyncSender<bool>) {
        let _ = ready.send(false);
    }
    pub fn rebind(_: HotkeyBinding, _: bool) -> Result<(), BindError> {
        Err(BindError::Unavailable)
    }
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

#[cfg(test)]
mod chord_tests {
    use super::*;
    static CAPTURE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    use jot_core::hotkey::{DictationActivationMode, HotkeyIntent, HotkeyProcessor};
    const CTRL: u32 = 0xA2;
    const ALT: u32 = 0xA4;
    const SHIFT: u32 = 0xA0;
    const E: u32 = 0x45;
    const BINDING: HotkeyBinding = HotkeyBinding::Chord(HotkeyChord {
        modifiers: 3,
        vk: E,
    });

    fn physical(
        keyboard: &mut PhysicalKeyboard,
        processor: &mut HotkeyProcessor,
        vk: u32,
        down: bool,
        at: u64,
    ) -> Vec<HotkeyIntent> {
        keyboard
            .handle(vk, down, Some(BINDING), true, false)
            .event
            .map(|event| processor.handle(event, Duration::from_millis(at)).intents)
            .unwrap_or_default()
    }

    #[test]
    fn chords_finalize_once_for_all_six_release_orders_and_repeat() {
        let _guard = CAPTURE_TEST_LOCK.lock().unwrap();
        for order in [
            [CTRL, ALT, E],
            [CTRL, E, ALT],
            [ALT, CTRL, E],
            [ALT, E, CTRL],
            [E, ALT, CTRL],
            [E, CTRL, ALT],
        ] {
            let mut keyboard = PhysicalKeyboard::default();
            let mut processor = HotkeyProcessor::new();
            processor.chord_binding = true;
            assert!(physical(&mut keyboard, &mut processor, CTRL, true, 0).is_empty());
            assert!(physical(&mut keyboard, &mut processor, ALT, true, 1).is_empty());
            assert_eq!(
                physical(&mut keyboard, &mut processor, E, true, 2),
                vec![HotkeyIntent::Begin]
            );
            assert!(physical(&mut keyboard, &mut processor, E, true, 3).is_empty());
            assert!(physical(&mut keyboard, &mut processor, 0x41, true, 4).is_empty());
            assert_eq!(
                physical(&mut keyboard, &mut processor, order[0], false, 5),
                vec![HotkeyIntent::Finalize]
            );
            assert!(physical(&mut keyboard, &mut processor, order[1], false, 6).is_empty());
            assert!(physical(&mut keyboard, &mut processor, order[2], false, 7).is_empty());
            assert!(!processor.is_session_active());
        }
    }

    #[test]
    fn toggle_chords_release_without_finalizing_and_allow_two_cycles() {
        let _guard = CAPTURE_TEST_LOCK.lock().unwrap();
        let mut keyboard = PhysicalKeyboard::default();
        let mut processor = HotkeyProcessor::new();
        processor.activation_mode = DictationActivationMode::Toggle;
        processor.chord_binding = true;
        for cycle in 0..2 {
            for step in 0..2 {
                let at = cycle * 100 + step * 20;
                physical(&mut keyboard, &mut processor, CTRL, true, at);
                physical(&mut keyboard, &mut processor, ALT, true, at + 1);
                assert_eq!(
                    physical(&mut keyboard, &mut processor, E, true, at + 2),
                    vec![if step == 0 {
                        HotkeyIntent::Begin
                    } else {
                        HotkeyIntent::Finalize
                    }]
                );
                assert!(physical(&mut keyboard, &mut processor, E, true, at + 3).is_empty());
                assert_eq!(
                    physical(&mut keyboard, &mut processor, ALT, false, at + 4),
                    if step == 0 {
                        vec![HotkeyIntent::LockIn]
                    } else {
                        vec![]
                    }
                );
                physical(&mut keyboard, &mut processor, E, false, at + 5);
                physical(&mut keyboard, &mut processor, CTRL, false, at + 6);
            }
            assert!(!processor.is_session_active());
        }
    }

    #[test]
    fn partial_extra_modifier_and_modifier_repress_do_not_trigger() {
        let _guard = CAPTURE_TEST_LOCK.lock().unwrap();
        let mut keyboard = PhysicalKeyboard::default();
        assert_eq!(
            keyboard.handle(E, true, Some(BINDING), false, false),
            Decision::IGNORE
        );
        keyboard.handle(CTRL, true, Some(BINDING), false, false);
        keyboard.handle(ALT, true, Some(BINDING), false, false);
        assert_eq!(
            keyboard.handle(E, true, Some(BINDING), false, false),
            Decision::IGNORE
        );
        keyboard.handle(E, false, Some(BINDING), false, false);
        keyboard.handle(SHIFT, true, Some(BINDING), false, false);
        assert_eq!(
            keyboard.handle(E, true, Some(BINDING), false, false),
            Decision::IGNORE
        );
        keyboard.handle(E, false, Some(BINDING), false, false);
        keyboard.handle(SHIFT, false, Some(BINDING), false, false);
        assert_eq!(
            keyboard.handle(E, true, Some(BINDING), false, false).event,
            Some(HotkeyEvent::HotkeyDown)
        );
        assert_eq!(
            keyboard
                .handle(CTRL, false, Some(BINDING), false, false)
                .event,
            Some(HotkeyEvent::HotkeyUp)
        );
        keyboard.handle(CTRL, true, Some(BINDING), false, false);
        assert_eq!(
            keyboard.handle(E, true, Some(BINDING), false, false).event,
            None
        );
        assert!(
            keyboard
                .handle(E, false, Some(BINDING), false, false)
                .swallow
        );
    }

    #[test]
    fn refused_begin_and_escape_require_a_fresh_physical_press() {
        let _guard = CAPTURE_TEST_LOCK.lock().unwrap();
        for cancelled in [false, true] {
            let mut keyboard = PhysicalKeyboard::default();
            let mut processor = HotkeyProcessor::new();
            processor.chord_binding = true;
            physical(&mut keyboard, &mut processor, CTRL, true, 0);
            physical(&mut keyboard, &mut processor, ALT, true, 1);
            assert_eq!(
                physical(&mut keyboard, &mut processor, E, true, 2),
                vec![HotkeyIntent::Begin]
            );
            if cancelled {
                assert_eq!(
                    physical(&mut keyboard, &mut processor, VK_ESCAPE, true, 3),
                    vec![HotkeyIntent::Cancel]
                );
            } else {
                processor.reset();
            }
            assert!(physical(&mut keyboard, &mut processor, E, true, 4).is_empty());
            assert!(physical(&mut keyboard, &mut processor, E, false, 5).is_empty());
            assert_eq!(
                physical(&mut keyboard, &mut processor, E, true, 6),
                vec![HotkeyIntent::Begin]
            );
        }
        let mut keyboard = PhysicalKeyboard::default();
        assert!(
            !keyboard
                .handle(VK_ESCAPE, true, Some(BINDING), false, false)
                .swallow
        );
    }

    #[test]
    fn mode_and_binding_change_preserve_the_pressed_chord_release() {
        let _guard = CAPTURE_TEST_LOCK.lock().unwrap();
        let mut keyboard = PhysicalKeyboard::default();
        let mut processor = HotkeyProcessor::new();
        processor.chord_binding = true;
        physical(&mut keyboard, &mut processor, CTRL, true, 0);
        physical(&mut keyboard, &mut processor, ALT, true, 1);
        physical(&mut keyboard, &mut processor, E, true, 2);
        processor.activation_mode = DictationActivationMode::Toggle;
        processor.chord_binding = false;
        let release = keyboard.handle(
            ALT,
            false,
            Some(HotkeyBinding::Bare(HotkeyKey::RightControl)),
            false,
            false,
        );
        assert_eq!(
            release
                .event
                .map(|event| processor.handle(event, Duration::from_millis(3)).intents),
            Some(vec![HotkeyIntent::Finalize])
        );
    }
    #[test]
    fn capture_requires_confirmation_allows_keyboard_navigation_and_escape() {
        let _guard = CAPTURE_TEST_LOCK.lock().unwrap();
        let mut keyboard = PhysicalKeyboard::default();
        start_capture();
        for vk in [CTRL, ALT, VK_SPACE] {
            let decision = keyboard.handle(vk, true, Some(BINDING), false, false);
            assert!(decision.swallow);
            assert_eq!(decision.event, None, "capture must not arm a dictation");
        }
        assert_eq!(
            captured_binding(),
            Some(Ok(HotkeyBinding::Chord(HotkeyChord {
                modifiers: 3,
                vk: VK_SPACE
            })))
        );
        for vk in [VK_SPACE, ALT, CTRL] {
            keyboard.handle(vk, false, Some(BINDING), false, false);
        }
        assert_eq!(
            keyboard.handle(0x09, true, Some(BINDING), false, false),
            Decision::IGNORE,
            "Tab must reach confirmation controls"
        );
        assert_eq!(
            keyboard
                .handle(VK_ESCAPE, true, Some(BINDING), false, false)
                .event,
            None
        );
        assert_eq!(captured_binding(), Some(Err(())));
        stop_capture();
        keyboard.handle(0x09, false, Some(BINDING), false, false);
        keyboard.handle(VK_ESCAPE, false, Some(BINDING), false, false);
        start_capture();
        keyboard.handle(0xA3, true, Some(BINDING), false, false);
        keyboard.handle(0xA3, false, Some(BINDING), false, false);
        assert_eq!(
            captured_binding(),
            Some(Ok(HotkeyBinding::Bare(HotkeyKey::RightControl)))
        );
        stop_capture();
    }
}
