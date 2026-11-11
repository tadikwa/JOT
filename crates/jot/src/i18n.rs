//! UI message IDs and language selection. Interface language does not affect dictation.
use jot_core::hotkey::{HotkeyBinding, HotkeyChord, HotkeyKey};
use jot_core::settings::UiLanguage;
use std::fmt::Display;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Locale {
    En,
    Fr,
}

static FRENCH: AtomicBool = AtomicBool::new(false);

fn resolve(preference: UiLanguage, windows_ui_language: u16) -> Locale {
    match preference {
        UiLanguage::French => Locale::Fr,
        UiLanguage::English => Locale::En,
        // Windows encodes the primary language in the low ten bits.
        UiLanguage::Automatic if windows_ui_language & 0x03ff == 0x0c => Locale::Fr,
        UiLanguage::Automatic => Locale::En,
    }
}

pub fn select(preference: UiLanguage) {
    #[cfg(windows)]
    let system = unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() };
    #[cfg(not(windows))]
    let system = 0;
    FRENCH.store(resolve(preference, system) == Locale::Fr, Ordering::Relaxed);
}

pub fn locale() -> Locale {
    if FRENCH.load(Ordering::Relaxed) {
        Locale::Fr
    } else {
        Locale::En
    }
}

macro_rules! messages {
    ($($id:ident => $en:literal $(, $fr:literal)?;)*) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Msg { $($id,)* }

        impl Msg {
            pub fn text(self, locale: Locale) -> &'static str {
                let (english, french) = match self {
                    $(Self::$id => ($en, messages!(@optional $($fr)?)),)*
                };
                fallback(locale, english, french)
            }

            #[cfg(test)]
            const ALL: &[Self] = &[$(Self::$id,)*];
        }
    };
    (@optional $fr:literal) => { Some($fr) };
    (@optional) => { None };
}

include!("messages.rs");

fn fallback(locale: Locale, english: &'static str, french: Option<&'static str>) -> &'static str {
    match locale {
        Locale::Fr => french.filter(|text| !text.is_empty()).unwrap_or(english),
        Locale::En => english,
    }
}

pub fn tr(message: Msg) -> &'static str {
    message.text(locale())
}

pub fn tf(message: Msg, values: &[&dyn Display]) -> String {
    interpolate(tr(message), values)
}

fn interpolate(template: &str, values: &[&dyn Display]) -> String {
    use std::fmt::Write as _;
    let mut result = String::new();
    for part in template.split_inclusive('}') {
        if let Some((prefix, index)) = part.strip_suffix('}').and_then(|s| s.rsplit_once('{'))
            && let Ok(index) = index.parse::<usize>()
            && let Some(value) = values.get(index)
        {
            result.push_str(prefix);
            let _ = write!(result, "{value}");
        } else {
            result.push_str(part);
        }
    }
    result
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalizedText {
    message: Msg,
    values: Vec<String>,
}

impl From<Msg> for LocalizedText {
    fn from(message: Msg) -> Self {
        Self {
            message,
            values: Vec::new(),
        }
    }
}

impl LocalizedText {
    pub fn with(message: Msg, values: &[&dyn Display]) -> Self {
        Self {
            message,
            values: values.iter().map(ToString::to_string).collect(),
        }
    }

    pub fn render(&self) -> gpui::SharedString {
        self.render_in(locale())
    }

    pub fn render_in(&self, locale: Locale) -> gpui::SharedString {
        let template = self.message.text(locale);
        if self.values.is_empty() {
            return template.into();
        }
        let args: Vec<&dyn Display> = self.values.iter().map(|arg| arg as &dyn Display).collect();
        interpolate(template, &args).into()
    }
}

pub fn hotkey(key: HotkeyKey) -> &'static str {
    tr(match key {
        HotkeyKey::RightControl => Msg::RightCtrl,
        HotkeyKey::RightAlt => Msg::RightAlt,
        HotkeyKey::RightShift => Msg::RightShift,
        HotkeyKey::RightWin => Msg::RightWin,
        HotkeyKey::CapsLock => Msg::CapsLock,
    })
}

pub fn hotkey_binding(binding: HotkeyBinding) -> String {
    let HotkeyBinding::Chord(chord) = binding else {
        let HotkeyBinding::Bare(key) = binding else {
            unreachable!()
        };
        return hotkey(key).to_string();
    };
    let mut parts = Vec::new();
    if chord.modifiers & HotkeyChord::CTRL != 0 {
        parts.push("Ctrl".to_string());
    }
    if chord.modifiers & HotkeyChord::ALT != 0 {
        parts.push("Alt".to_string());
    }
    if chord.modifiers & HotkeyChord::SHIFT != 0 {
        parts.push(tr(Msg::ShiftModifier).to_string());
    }
    if chord.modifiers & HotkeyChord::WIN != 0 {
        parts.push("Windows".to_string());
    }
    parts.push(match chord.vk {
        0x20 => tr(Msg::SpaceKey).to_string(),
        0x30..=0x39 | 0x41..=0x5A => char::from_u32(chord.vk).unwrap().to_string(),
        0x70..=0x87 => format!("F{}", chord.vk - 0x6f),
        _ => format!("VK {:02X}", chord.vk),
    });
    parts.join(" + ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_uses_display_language_and_explicit_choices_take_precedence() {
        for french in [0x040c, 0x0c0c, 0x100c] {
            assert_eq!(resolve(UiLanguage::Automatic, french), Locale::Fr);
            assert_eq!(resolve(UiLanguage::English, french), Locale::En);
        }
        assert_eq!(resolve(UiLanguage::Automatic, 0x0409), Locale::En);
        assert_eq!(resolve(UiLanguage::Automatic, 0), Locale::En);
        assert_eq!(resolve(UiLanguage::French, 0x0409), Locale::Fr);
    }

    #[test]
    fn a_missing_or_empty_translation_uses_english() {
        assert_eq!(fallback(Locale::Fr, "English", None), "English");
        assert_eq!(fallback(Locale::Fr, "English", Some("")), "English");
    }

    #[test]
    fn catalog_preserves_template_arguments() {
        fn arguments(text: &str) -> Vec<usize> {
            let mut indices: Vec<_> = text
                .split_inclusive('}')
                .filter_map(|part| part.strip_suffix('}')?.rsplit_once('{')?.1.parse().ok())
                .collect();
            indices.sort_unstable();
            indices
        }
        for &message in Msg::ALL {
            let en = message.text(Locale::En);
            let fr = message.text(Locale::Fr);
            assert!(!en.is_empty());
            assert_eq!(arguments(en), arguments(fr), "{message:?}");
        }
        assert_eq!(interpolate("{1} / {0}", &[&"{1}", &"texte"]), "texte / {1}");
    }
}
