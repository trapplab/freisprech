//! Dictation language: turns the setting into the `language` option for the ASR.

/// Setting: the model detects the language from speech (the model's own `"auto"`).
pub const DETECT: &str = "auto";
/// Setting: the system language, if the model supports it, else detect from speech.
pub const SYSTEM: &str = "system";

/// Locales of the multilingual Nemotron model (the list VS Code's dictation uses).
const MODEL_LOCALES: &[&str] = &[
    "ar-AR", "bg-BG", "cs-CZ", "da-DK", "de-DE", "en-GB", "en-US", "es-ES", "es-US", "et-EE",
    "fi-FI", "fr-CA", "fr-FR", "el-GR", "he-IL", "hi-IN", "hr-HR", "hu-HU", "it-IT", "ja-JP",
    "ko-KR", "lt-LT", "lv-LV", "mt-MT", "nb-NO", "nl-NL", "nn-NO", "pl-PL", "pt-BR", "pt-PT",
    "ro-RO", "ru-RU", "sk-SK", "sl-SI", "sv-SE", "th-TH", "tr-TR", "uk-UA", "vi-VN", "zh-CN",
];

/// Locale for a language with several regions, when the system region isn't one of them.
/// Other languages take their only locale from [`MODEL_LOCALES`].
const DEFAULT_LOCALES: &[(&str, &str)] = &[
    ("en", "en-US"),
    ("es", "es-US"),
    ("fr", "fr-FR"),
    ("pt", "pt-PT"),
];

/// The `language` option for a setting: [`SYSTEM`] becomes the system locale (like VS Code
/// does) or, if the model doesn't know it, [`DETECT`]; anything else (`"auto"`, `"de"`,
/// `"en-GB"`) is passed on.
pub fn resolve(setting: &str) -> String {
    resolve_with(setting, system_locale().as_deref())
}

fn resolve_with(setting: &str, system: Option<&str>) -> String {
    match setting {
        SYSTEM => system.and_then(model_locale).unwrap_or(DETECT).to_owned(),
        other => other.to_owned(),
    }
}

/// `de_DE.UTF-8`, `de-AT` or `de` -> `de-DE`; `None` if the model doesn't know the language.
fn model_locale(locale: &str) -> Option<&'static str> {
    let locale = locale.split(['.', '@']).next()?.replace('_', "-");
    let (lang, region) = locale.split_once('-').unwrap_or((&locale, ""));
    let lang = lang.to_ascii_lowercase();
    let full = format!("{lang}-{}", region.to_ascii_uppercase());
    MODEL_LOCALES
        .iter()
        .find(|l| **l == full)
        .or_else(|| {
            DEFAULT_LOCALES
                .iter()
                .find(|(l, _)| *l == lang)
                .map(|(_, l)| l)
        })
        .or_else(|| {
            MODEL_LOCALES
                .iter()
                .find(|l| l.split('-').next() == Some(&lang))
        })
        .copied()
}

/// Locale of the session, e.g. `de_DE.UTF-8`.
#[cfg(not(windows))]
fn system_locale() -> Option<String> {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .filter_map(|var| std::env::var(var).ok())
        .find(|value| !value.is_empty())
}

/// The user's regional format, e.g. `de-DE`. Chosen over the display language: people often
/// run an English Windows but speak the language of their region.
#[cfg(windows)]
fn system_locale() -> Option<String> {
    use windows_sys::Win32::Globalization::GetUserDefaultLocaleName;

    // LOCALE_NAME_MAX_LENGTH
    let mut buf = [0u16; 85];
    // SAFETY: `buf` is writable for the given number of UTF-16 units.
    let len = unsafe { GetUserDefaultLocaleName(buf.as_mut_ptr(), buf.len() as i32) };
    // `len` counts the terminating null; 0 means failure.
    (len > 1).then(|| String::from_utf16_lossy(&buf[..len as usize - 1]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_setting_uses_the_system_locale() {
        assert_eq!(resolve_with(SYSTEM, Some("de_DE.UTF-8")), "de-DE");
        assert_eq!(resolve_with(SYSTEM, Some("de-AT")), "de-DE");
        assert_eq!(resolve_with(SYSTEM, Some("en_GB.UTF-8")), "en-GB");
        assert_eq!(resolve_with(SYSTEM, Some("en_IE")), "en-US");
        assert_eq!(resolve_with(SYSTEM, Some("pt_BR")), "pt-BR");
        assert_eq!(resolve_with(SYSTEM, Some("sv")), "sv-SE");
        assert_eq!(resolve_with(SYSTEM, Some("sr@latin")), "auto");
    }

    #[test]
    fn system_setting_falls_back_to_detection() {
        assert_eq!(resolve_with(SYSTEM, Some("C.UTF-8")), "auto");
        assert_eq!(resolve_with(SYSTEM, Some("POSIX")), "auto");
        assert_eq!(resolve_with(SYSTEM, None), "auto");
    }

    #[test]
    fn other_settings_are_passed_on() {
        assert_eq!(resolve_with(DETECT, Some("de_DE.UTF-8")), "auto");
        assert_eq!(resolve_with("de", Some("en_US.UTF-8")), "de");
    }
}
