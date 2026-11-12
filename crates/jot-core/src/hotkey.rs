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

//! The hotkey key set and the pure hold/lock/cancel grammar.
//!
//! Windows has no `fn` key: it is handled in keyboard firmware and never
//! reaches `WH_KEYBOARD_LL`, so the macOS default has no equivalent here. The
//! set below is instead the bare keys a low-level hook *can* see and swallow.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// A bare-modifier dictation key, identified by Windows virtual-key code.
///
/// These are keys `RegisterHotKey` cannot bind on their own (it requires a
/// modifier + a normal key) — hence the low-level keyboard hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HotkeyKey {
    RightControl,
    RightAlt,
    RightShift,
    RightWin,
    /// Held like `fn` and in the same corner. Requires the hook to swallow the
    /// key so the shift-lock state never toggles while dictating.
    CapsLock,
}

impl Default for HotkeyKey {
    /// Right Ctrl: reachable with the right thumb/pinky, almost never used bare,
    /// and unlike Caps Lock it carries no toggle state to suppress.
    fn default() -> Self {
        HotkeyKey::RightControl
    }
}

impl HotkeyKey {
    pub const ALL: [HotkeyKey; 5] = [
        HotkeyKey::RightControl,
        HotkeyKey::RightAlt,
        HotkeyKey::RightShift,
        HotkeyKey::RightWin,
        HotkeyKey::CapsLock,
    ];

    /// Windows virtual-key code as reported by `KBDLLHOOKSTRUCT::vkCode`.
    pub fn vk_code(self) -> u32 {
        match self {
            HotkeyKey::RightControl => 0xA3, // VK_RCONTROL
            HotkeyKey::RightAlt => 0xA5,     // VK_RMENU
            HotkeyKey::RightShift => 0xA1,   // VK_RSHIFT
            HotkeyKey::RightWin => 0x5C,     // VK_RWIN
            HotkeyKey::CapsLock => 0x14,     // VK_CAPITAL
        }
    }

    /// True when this key must be swallowed by the hook rather than passed on.
    ///
    /// Caps Lock toggles a global state, and Right Win opens the Start menu on
    /// release — both would fire on every dictation. The plain modifiers are
    /// passed through: swallowing them would break chords the user still types.
    pub fn must_suppress(self) -> bool {
        matches!(self, HotkeyKey::CapsLock | HotkeyKey::RightWin)
    }

    pub fn display_name(self) -> &'static str {
        match self {
            HotkeyKey::RightControl => "Right Ctrl",
            HotkeyKey::RightAlt => "Right Alt",
            HotkeyKey::RightShift => "Right Shift",
            HotkeyKey::RightWin => "Right Win",
            HotkeyKey::CapsLock => "Caps Lock",
        }
    }

    pub fn from_vk(vk: u32) -> Option<Self> {
        Self::ALL.into_iter().find(|key| key.vk_code() == vk)
    }
}

/// Windows modifier bits, deliberately stable in settings JSON.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HotkeyChord {
    pub modifiers: u32,
    pub vk: u32,
}

impl HotkeyChord {
    pub const ALT: u32 = 1;
    pub const CTRL: u32 = 2;
    pub const SHIFT: u32 = 4;
    pub const WIN: u32 = 8;
    /// Sided bits, used only by modifier-only chords (`vk == 0`). Older builds
    /// reject them as invalid instead of dropping the whole settings file.
    pub const SIDED: u32 = 0xFF00;
    /// (bit, virtual-key code) of each sided modifier, in display order.
    pub const SIDES: [(u32, u32); 8] = [
        (0x100, 0xA2),  // left Ctrl
        (0x200, 0xA3),  // right Ctrl
        (0x400, 0xA4),  // left Alt
        (0x800, 0xA5),  // right Alt
        (0x1000, 0xA0), // left Shift
        (0x2000, 0xA1), // right Shift
        (0x4000, 0x5B), // left Win
        (0x8000, 0x5C), // right Win
    ];

    pub fn sided_bit(vk: u32) -> Option<u32> {
        Self::SIDES
            .iter()
            .find(|(_, side)| *side == vk)
            .map(|(bit, _)| *bit)
    }

    pub fn is_modifier_only(self) -> bool {
        self.vk == 0
    }

    pub fn validate(self) -> Result<(), ChordProblem> {
        if self.is_modifier_only() {
            if self.modifiers & !Self::SIDED != 0 {
                return Err(ChordProblem::UnsupportedKey);
            }
            return match self.modifiers.count_ones() {
                2 | 3 => Ok(()),
                _ => Err(ChordProblem::NeedsTwoKeys),
            };
        }
        // Windows-logo + key combinations belong to the OS.
        if self.modifiers & Self::WIN != 0 {
            return Err(ChordProblem::WindowsKey);
        }
        if self.modifiers == 0 || self.modifiers & !7 != 0 {
            return Err(ChordProblem::NeedsModifier);
        }
        // F12 belongs to the debugger; Tab/Esc/Enter/Backspace drive the OS and dialogs.
        if !matches!(
            self.vk,
            0x20..=0x28
                | 0x2D
                | 0x2E
                | 0x30..=0x39
                | 0x41..=0x5A
                | 0x60..=0x6F
                | 0x70..=0x7A
                | 0x7C..=0x87
                | 0xBA..=0xC0
                | 0xDB..=0xDF
                | 0xE2
        ) {
            return Err(ChordProblem::UnsupportedKey);
        }
        if self.modifiers == Self::ALT && self.vk == 0x20 {
            return Err(ChordProblem::SystemMenu);
        }
        // Alt+F4 closes windows; Ctrl+Alt+Del (main or keypad) is the secure attention sequence.
        let ctrl_alt = Self::CTRL | Self::ALT;
        if self.modifiers == Self::ALT && self.vk == 0x73
            || self.modifiers & ctrl_alt == ctrl_alt && matches!(self.vk, 0x2E | 0x6E)
        {
            return Err(ChordProblem::Reserved);
        }
        Ok(())
    }
}

/// Why a shortcut cannot be a dictation binding; the UI explains each one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChordProblem {
    NeedsModifier,
    WindowsKey,
    UnsupportedKey,
    /// Alt+Space opens every window's system menu.
    SystemMenu,
    Reserved,
    NeedsTwoKeys,
}

impl std::fmt::Display for ChordProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NeedsModifier => "use Ctrl, Alt or Shift with a normal key",
            Self::WindowsKey => "Windows-key combinations with a normal key belong to Windows",
            Self::UnsupportedKey => "unsupported key",
            Self::SystemMenu => "Alt+Space opens the window menu",
            Self::Reserved => "reserved shortcut",
            Self::NeedsTwoKeys => "use two or three modifier keys",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyBinding {
    Bare(HotkeyKey),
    Chord(HotkeyChord),
}

impl HotkeyBinding {
    pub fn vk_code(self) -> u32 {
        match self {
            Self::Bare(key) => key.vk_code(),
            Self::Chord(chord) => chord.vk,
        }
    }

    pub fn encode(self) -> u64 {
        match self {
            Self::Bare(key) => u64::from(key.vk_code()),
            Self::Chord(chord) => {
                (1u64 << 32) | (u64::from(chord.modifiers) << 16) | u64::from(chord.vk)
            }
        }
    }

    pub fn decode(value: u64) -> Option<Self> {
        let vk = (value & 0xffff) as u32;
        if value & (1u64 << 32) == 0 {
            HotkeyKey::from_vk(vk).map(Self::Bare)
        } else {
            Some(Self::Chord(HotkeyChord {
                vk,
                modifiers: ((value >> 16) & 0xffff) as u32,
            }))
        }
    }
}

/// How the configured key starts and stops a dictation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DictationActivationMode {
    #[default]
    Hold,
    Toggle,
}

/// What the hotkey layer asks the session coordinator to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyIntent {
    /// Key went down — start recording NOW (audio from t=0, before classification).
    Begin,
    /// Double-tap detected — lock into hands-free recording.
    LockIn,
    /// Hold released (or stop requested while locked) — finalize and transcribe.
    Finalize,
    /// Esc — cancel the session (audio still saved per retention policy).
    Cancel,
    /// Single short tap with no second tap — cancel quietly and show the
    /// "Hold to talk — tap Space to lock" coaching hint (never an error sound).
    ShortTapHint,
    /// Another key was typed within the interruption window — accidental chord,
    /// cancel silently.
    AbortAccidental,
}

/// Timing constants for the hotkey grammar.
pub mod tuning {
    use std::time::Duration;

    /// Press shorter than this is a "tap"; at/above is a hold (push-to-talk).
    pub const HOLD_THRESHOLD: Duration = Duration::from_millis(300);
    /// Max gap between the first tap's key-up and the second tap's key-down to
    /// count as a double-tap.
    pub const DOUBLE_TAP_WINDOW: Duration = Duration::from_millis(500);
    /// A non-hotkey keystroke within this window of session start aborts as accidental.
    pub const INTERRUPTION_WINDOW: Duration = Duration::from_millis(1000);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyEvent {
    HotkeyDown,
    HotkeyUp,
    EscDown,
    OtherKeyDown,
    /// Space pressed while the hotkey is physically held — the timing-free
    /// hands-free gesture ("hold, tap Space, let go").
    SpaceLock,
    /// The double-tap window expired (fed back by the timer the caller armed).
    DoubleTapTimeout,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Effects {
    pub intents: Vec<HotkeyIntent>,
    /// Arm the double-tap timer to fire after this duration (`None` = leave as-is).
    pub arm_timer: Option<Duration>,
    pub disarm_timer: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Phase {
    #[default]
    Idle,
    /// Key physically down, classification pending (hold vs tap).
    Pressed {
        down_at: Duration,
        mode: DictationActivationMode,
        chord: bool,
        double_tap: bool,
    },
    /// First short tap released; waiting for a possible second tap. Still recording.
    PendingSecondTap { session_start_at: Duration },
    /// Hands-free.
    Locked,
}

/// The pure hotkey grammar. In Toggle, the first press starts recording, its
/// release locks it hands-free, and the next distinct press finalizes.
/// The legacy Hold gestures are:
///
/// ```text
///   hold >= 0.3s           -> push-to-talk: key-up finalizes
///   tap, tap (<= 0.5s gap) -> hands-free lock: press again finalizes
///   single tap             -> coaching hint, session quietly cancelled
///   Esc                    -> cancel
///   other key < 1s in      -> accidental chord, silent abort
/// ```
///
/// Recording ALWAYS starts on the first key-down (`Begin`) so no audio is ever
/// lost while the grammar disambiguates. Between the taps of a double-tap the
/// session keeps recording.
///
/// Pure and clock-free: callers pass monotonic timestamps; timers are returned
/// as effects and fed back in as `DoubleTapTimeout`.
#[derive(Debug, Default)]
pub struct HotkeyProcessor {
    phase: Phase,
    /// Keep tracking the physical press even if the gesture ends before key-up.
    key_down: bool,
    pub activation_mode: DictationActivationMode,
    /// Chords have an unambiguous release: even a short hold finalizes.
    pub chord_binding: bool,
    /// When off, a short tap hints immediately and never arms the double-tap
    /// window. Default OFF: firm taps routinely exceed the hold threshold,
    /// misreading tap-tap as hold→finalize. Space-while-holding replaced it.
    pub double_tap_lock_enabled: bool,
}

impl HotkeyProcessor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget the gesture when the coordinator refuses or ends a recording.
    /// A physical key-up is still required before accepting a fresh press.
    pub fn reset(&mut self) {
        self.phase = Phase::Idle;
    }

    pub fn is_key_down(&self) -> bool {
        self.key_down
    }

    /// Whether Space currently means the hands-free gesture.
    pub fn can_space_lock(&self) -> bool {
        matches!(
            self.phase,
            Phase::Pressed {
                mode: DictationActivationMode::Hold,
                chord: false,
                ..
            }
        )
    }

    /// True whenever a dictation session is in flight from the hotkey's
    /// perspective — the hook uses this to decide whether to intercept Esc.
    pub fn is_session_active(&self) -> bool {
        self.phase != Phase::Idle
    }

    pub fn handle(&mut self, event: HotkeyEvent, now: Duration) -> Effects {
        use HotkeyEvent as E;
        let mut fx = Effects::default();
        match event {
            E::HotkeyDown if self.key_down => return fx,
            E::HotkeyDown => self.key_down = true,
            E::HotkeyUp if !self.key_down => return fx,
            E::HotkeyUp => self.key_down = false,
            _ => {}
        }

        match (self.phase, event) {
            // ----- idle
            (Phase::Idle, E::HotkeyDown) => {
                self.phase = Phase::Pressed {
                    down_at: now,
                    mode: self.activation_mode,
                    chord: self.chord_binding,
                    double_tap: self.double_tap_lock_enabled,
                };
                fx.intents.push(HotkeyIntent::Begin);
            }
            (Phase::Idle, _) => {}

            // ----- pressed (key down, disambiguating)
            (
                Phase::Pressed {
                    mode: DictationActivationMode::Toggle,
                    ..
                },
                E::HotkeyUp,
            ) => {
                self.phase = Phase::Locked;
                fx.intents.push(HotkeyIntent::LockIn);
            }
            (
                Phase::Pressed {
                    down_at,
                    chord,
                    double_tap,
                    ..
                },
                E::HotkeyUp,
            ) => {
                if chord || now.saturating_sub(down_at) >= tuning::HOLD_THRESHOLD {
                    self.phase = Phase::Idle;
                    fx.intents.push(HotkeyIntent::Finalize);
                } else if double_tap {
                    self.phase = Phase::PendingSecondTap {
                        session_start_at: down_at,
                    };
                    fx.arm_timer = Some(tuning::DOUBLE_TAP_WINDOW);
                } else {
                    self.phase = Phase::Idle;
                    fx.intents.push(HotkeyIntent::ShortTapHint);
                }
            }
            (Phase::Pressed { .. }, E::EscDown) => {
                self.phase = Phase::Idle;
                fx.intents.push(HotkeyIntent::Cancel);
            }
            (Phase::Pressed { down_at, .. }, E::OtherKeyDown) => {
                if now.saturating_sub(down_at) < tuning::INTERRUPTION_WINDOW {
                    self.phase = Phase::Idle;
                    fx.intents.push(HotkeyIntent::AbortAccidental);
                }
                // After the window: the user is deliberately chording/typing
                // mid-hold — keep going.
            }
            // Hold + tap Space = hands-free, no timing window. The hotkey release
            // that follows belongs to this gesture and must not finalize.
            (
                Phase::Pressed {
                    mode: DictationActivationMode::Hold,
                    ..
                },
                E::SpaceLock,
            ) => {
                self.phase = Phase::Locked;
                fx.intents.push(HotkeyIntent::LockIn);
            }
            (Phase::Pressed { .. }, E::HotkeyDown | E::DoubleTapTimeout | E::SpaceLock) => {}

            // ----- pendingSecondTap (short tap released, window open, still recording)
            (Phase::PendingSecondTap { .. }, E::HotkeyDown) => {
                self.phase = Phase::Locked;
                fx.intents.push(HotkeyIntent::LockIn);
                fx.disarm_timer = true;
            }
            (Phase::PendingSecondTap { .. }, E::DoubleTapTimeout) => {
                self.phase = Phase::Idle;
                fx.intents.push(HotkeyIntent::ShortTapHint);
            }
            (Phase::PendingSecondTap { .. }, E::EscDown) => {
                self.phase = Phase::Idle;
                fx.intents.push(HotkeyIntent::Cancel);
                fx.disarm_timer = true;
            }
            (Phase::PendingSecondTap { session_start_at }, E::OtherKeyDown) => {
                self.phase = Phase::Idle;
                fx.disarm_timer = true;
                fx.intents.push(
                    if now.saturating_sub(session_start_at) < tuning::INTERRUPTION_WINDOW {
                        HotkeyIntent::AbortAccidental
                    } else {
                        HotkeyIntent::Cancel
                    },
                );
            }
            (Phase::PendingSecondTap { .. }, E::HotkeyUp | E::SpaceLock) => {}

            // ----- locked (hands-free)
            (Phase::Locked, E::HotkeyDown) => {
                self.phase = Phase::Idle;
                fx.intents.push(HotkeyIntent::Finalize);
            }
            (Phase::Locked, E::EscDown) => {
                self.phase = Phase::Idle;
                fx.intents.push(HotkeyIntent::Cancel);
            }
            (Phase::Locked, E::HotkeyUp | E::OtherKeyDown | E::DoubleTapTimeout | E::SpaceLock) => {
            }
        }

        fx
    }
}

#[cfg(test)]
mod tests {
    use super::HotkeyEvent as E;
    use super::HotkeyIntent as I;
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn hold_then_release_is_push_to_talk() {
        let mut p = HotkeyProcessor::new();
        assert_eq!(p.handle(E::HotkeyDown, ms(0)).intents, vec![I::Begin]);
        assert_eq!(p.handle(E::HotkeyUp, ms(900)).intents, vec![I::Finalize]);
        assert!(!p.is_session_active());
    }

    #[test]
    fn cancelling_a_hold_ignores_repeats_until_release() {
        let mut p = HotkeyProcessor::new();
        assert_eq!(p.handle(E::HotkeyDown, ms(0)).intents, vec![I::Begin]);
        assert!(p.handle(E::HotkeyDown, ms(500)).intents.is_empty());
        assert_eq!(p.handle(E::EscDown, ms(600)).intents, vec![I::Cancel]);
        assert!(p.handle(E::HotkeyDown, ms(650)).intents.is_empty());
        assert!(p.handle(E::HotkeyUp, ms(700)).intents.is_empty());
        assert_eq!(p.handle(E::HotkeyDown, ms(800)).intents, vec![I::Begin]);
        assert_eq!(p.handle(E::HotkeyUp, ms(1200)).intents, vec![I::Finalize]);
    }

    #[test]
    fn toggle_starts_and_stops_once_per_press_without_a_time_window() {
        let mut p = HotkeyProcessor::new();
        p.activation_mode = DictationActivationMode::Toggle;
        p.double_tap_lock_enabled = true;
        assert_eq!(p.handle(E::HotkeyDown, ms(0)).intents, vec![I::Begin]);
        assert!(!p.can_space_lock());
        assert_eq!(p.handle(E::HotkeyDown, ms(50)), Effects::default());
        assert_eq!(
            p.handle(E::HotkeyUp, ms(100)),
            Effects {
                intents: vec![I::LockIn],
                ..Effects::default()
            }
        );
        assert_eq!(p.handle(E::OtherKeyDown, ms(150)), Effects::default());
        assert_eq!(p.handle(E::DoubleTapTimeout, ms(700)), Effects::default());
        assert!(p.is_session_active());
        assert_eq!(
            p.handle(E::HotkeyDown, ms(60_000)).intents,
            vec![I::Finalize]
        );
        assert_eq!(p.handle(E::HotkeyDown, ms(60_100)), Effects::default());
        assert_eq!(p.handle(E::HotkeyUp, ms(60_200)), Effects::default());
        assert!(!p.is_session_active());
        assert_eq!(p.handle(E::HotkeyDown, ms(61_000)).intents, vec![I::Begin]);
    }

    #[test]
    fn toggle_can_be_cancelled_before_or_after_release() {
        for released in [false, true] {
            let mut p = HotkeyProcessor::new();
            p.activation_mode = DictationActivationMode::Toggle;
            p.handle(E::HotkeyDown, ms(0));
            if released {
                p.handle(E::HotkeyUp, ms(100));
            }
            assert_eq!(p.handle(E::EscDown, ms(200)).intents, vec![I::Cancel]);
            assert_eq!(p.handle(E::EscDown, ms(250)), Effects::default());
            if !released {
                assert_eq!(p.handle(E::HotkeyDown, ms(300)), Effects::default());
            }
            assert_eq!(p.handle(E::HotkeyUp, ms(350)), Effects::default());
            assert_eq!(p.handle(E::HotkeyDown, ms(500)).intents, vec![I::Begin]);
        }
    }

    #[test]
    fn a_mode_change_applies_to_the_next_dictation() {
        let mut p = HotkeyProcessor::new();
        p.handle(E::HotkeyDown, ms(0));
        p.activation_mode = DictationActivationMode::Toggle;
        assert!(p.can_space_lock());
        assert_eq!(p.handle(E::HotkeyUp, ms(400)).intents, vec![I::Finalize]);
        assert_eq!(p.handle(E::HotkeyDown, ms(500)).intents, vec![I::Begin]);
        p.activation_mode = DictationActivationMode::Hold;
        assert!(!p.can_space_lock());
        assert_eq!(p.handle(E::HotkeyUp, ms(900)).intents, vec![I::LockIn]);
        assert_eq!(p.handle(E::HotkeyDown, ms(1500)).intents, vec![I::Finalize]);
        p.handle(E::HotkeyUp, ms(1600));
        assert_eq!(p.handle(E::HotkeyDown, ms(1700)).intents, vec![I::Begin]);
        assert_eq!(p.handle(E::HotkeyUp, ms(2100)).intents, vec![I::Finalize]);
    }

    #[test]
    fn short_tap_coaches_when_double_tap_lock_is_off() {
        let mut p = HotkeyProcessor::new();
        p.handle(E::HotkeyDown, ms(0));
        let fx = p.handle(E::HotkeyUp, ms(100));
        assert_eq!(fx.intents, vec![I::ShortTapHint]);
        assert!(fx.arm_timer.is_none());
    }

    #[test]
    fn double_tap_locks_when_enabled() {
        let mut p = HotkeyProcessor::new();
        p.double_tap_lock_enabled = true;
        p.handle(E::HotkeyDown, ms(0));
        let fx = p.handle(E::HotkeyUp, ms(100));
        assert_eq!(fx.arm_timer, Some(tuning::DOUBLE_TAP_WINDOW));
        assert!(fx.intents.is_empty());
        let fx = p.handle(E::HotkeyDown, ms(300));
        assert_eq!(fx.intents, vec![I::LockIn]);
        assert!(fx.disarm_timer);
        // The release belonging to the lock gesture is swallowed.
        assert!(p.handle(E::HotkeyUp, ms(360)).intents.is_empty());
        // Pressing again finalizes.
        assert_eq!(p.handle(E::HotkeyDown, ms(5000)).intents, vec![I::Finalize]);
    }

    #[test]
    fn expired_double_tap_window_coaches() {
        let mut p = HotkeyProcessor::new();
        p.double_tap_lock_enabled = true;
        p.handle(E::HotkeyDown, ms(0));
        p.handle(E::HotkeyUp, ms(100));
        assert_eq!(
            p.handle(E::DoubleTapTimeout, ms(600)).intents,
            vec![I::ShortTapHint]
        );
    }

    #[test]
    fn space_while_holding_locks_and_swallows_the_release() {
        let mut p = HotkeyProcessor::new();
        p.handle(E::HotkeyDown, ms(0));
        assert_eq!(p.handle(E::SpaceLock, ms(120)).intents, vec![I::LockIn]);
        assert!(p.handle(E::HotkeyUp, ms(200)).intents.is_empty());
        assert!(p.is_session_active());
        assert_eq!(p.handle(E::HotkeyDown, ms(4000)).intents, vec![I::Finalize]);
    }

    #[test]
    fn other_key_inside_the_window_aborts_but_outside_it_does_not() {
        let mut p = HotkeyProcessor::new();
        p.handle(E::HotkeyDown, ms(0));
        assert_eq!(
            p.handle(E::OtherKeyDown, ms(500)).intents,
            vec![I::AbortAccidental]
        );

        let mut p = HotkeyProcessor::new();
        p.handle(E::HotkeyDown, ms(0));
        assert!(p.handle(E::OtherKeyDown, ms(1500)).intents.is_empty());
        assert!(p.is_session_active());
    }

    #[test]
    fn esc_cancels_and_swallows_the_trailing_release() {
        let mut p = HotkeyProcessor::new();
        p.handle(E::HotkeyDown, ms(0));
        assert_eq!(p.handle(E::EscDown, ms(200)).intents, vec![I::Cancel]);
        assert!(p.handle(E::HotkeyUp, ms(250)).intents.is_empty());
    }

    #[test]
    fn reset_unsticks_a_locked_phantom_session() {
        let mut p = HotkeyProcessor::new();
        p.handle(E::HotkeyDown, ms(0));
        p.handle(E::SpaceLock, ms(50));
        p.reset();
        assert!(!p.is_session_active());
        assert!(p.handle(E::HotkeyDown, ms(75)).intents.is_empty());
        assert!(p.handle(E::HotkeyUp, ms(90)).intents.is_empty());
        assert_eq!(p.handle(E::HotkeyDown, ms(100)).intents, vec![I::Begin]);
    }

    #[test]
    fn key_codes_round_trip() {
        for key in HotkeyKey::ALL {
            assert_eq!(HotkeyKey::from_vk(key.vk_code()), Some(key));
        }
        assert_eq!(HotkeyKey::from_vk(0x41), None);
    }
}

#[cfg(test)]
mod chord_binding_tests {
    use super::*;
    #[test]
    fn chords_are_stable_and_reject_reserved_keys() {
        for chord in [
            HotkeyChord {
                modifiers: 3,
                vk: 0x20,
            },
            HotkeyChord {
                modifiers: 6,
                vk: 0x44,
            },
        ] {
            chord.validate().unwrap();
            let binding = HotkeyBinding::Chord(chord);
            assert_eq!(HotkeyBinding::decode(binding.encode()), Some(binding));
            assert_eq!(
                serde_json::from_str::<HotkeyChord>(&serde_json::to_string(&chord).unwrap())
                    .unwrap(),
                chord
            );
        }
        for chord in [
            HotkeyChord {
                modifiers: 0,
                vk: 0x44,
            },
            HotkeyChord {
                modifiers: 8,
                vk: 0x44,
            },
            HotkeyChord {
                modifiers: 2,
                vk: 0x7b,
            },
            HotkeyChord {
                modifiers: 1,
                vk: 0x73,
            },
            HotkeyChord {
                modifiers: 1,
                vk: 0x20,
            },
            HotkeyChord {
                modifiers: 2,
                vk: 0x1b,
            },
            HotkeyChord {
                modifiers: 3,
                vk: 0x2e,
            },
            HotkeyChord {
                modifiers: 2,
                vk: 0xffff,
            },
        ] {
            assert!(chord.validate().is_err(), "{chord:?}");
        }
        for key in HotkeyKey::ALL {
            let binding = HotkeyBinding::Bare(key);
            assert_eq!(HotkeyBinding::decode(binding.encode()), Some(binding));
        }
    }
    #[test]
    fn modifier_only_chords_and_wider_keys_validate_with_a_reason() {
        let ok = |modifiers, vk| HotkeyChord { modifiers, vk }.validate();
        // Alt droit + Maj droite, Win gauche + Alt gauche, Ctrl gauche + Maj gauche + Alt gauche.
        assert_eq!(ok(0x800 | 0x2000, 0), Ok(()));
        assert_eq!(ok(0x4000 | 0x400, 0), Ok(()));
        assert_eq!(ok(0x100 | 0x1000 | 0x400, 0), Ok(()));
        assert_eq!(ok(0x200, 0), Err(ChordProblem::NeedsTwoKeys));
        assert_eq!(ok(0xF00, 0), Err(ChordProblem::NeedsTwoKeys));
        assert_eq!(
            ok(HotkeyChord::CTRL | 0x800, 0),
            Err(ChordProblem::UnsupportedKey)
        );
        // Ctrl+Maj+D, punctuation, keypad, arrows.
        for vk in [0x44, 0xBC, 0xDE, 0xE2, 0x60, 0x6B, 0x25, 0x2D] {
            assert_eq!(
                ok(HotkeyChord::CTRL | HotkeyChord::SHIFT, vk),
                Ok(()),
                "{vk:#x}"
            );
        }
        assert_eq!(ok(HotkeyChord::ALT, 0x20), Err(ChordProblem::SystemMenu));
        assert_eq!(ok(HotkeyChord::ALT, 0x73), Err(ChordProblem::Reserved));
        assert_eq!(ok(7, 0x2E), Err(ChordProblem::Reserved));
        assert_eq!(ok(3, 0x6E), Err(ChordProblem::Reserved));
        assert_eq!(
            ok(HotkeyChord::WIN | 2, 0x4C),
            Err(ChordProblem::WindowsKey)
        );
        for vk in [0x09, 0x0D, 0x08, 0x1B, 0x7B, 0x2C] {
            assert_eq!(ok(2, vk), Err(ChordProblem::UnsupportedKey), "{vk:#x}");
        }
        let binding = HotkeyBinding::Chord(HotkeyChord {
            modifiers: 0x4000 | 0x400,
            vk: 0,
        });
        assert_eq!(HotkeyBinding::decode(binding.encode()), Some(binding));
    }
    #[test]
    fn chord_hold_finalizes_even_when_released_quickly() {
        let mut processor = HotkeyProcessor::new();
        processor.chord_binding = true;
        assert_eq!(
            processor
                .handle(HotkeyEvent::HotkeyDown, Duration::ZERO)
                .intents,
            vec![HotkeyIntent::Begin]
        );
        assert!(!processor.can_space_lock());
        assert_eq!(
            processor
                .handle(HotkeyEvent::HotkeyUp, Duration::from_millis(10))
                .intents,
            vec![HotkeyIntent::Finalize]
        );
    }
}
