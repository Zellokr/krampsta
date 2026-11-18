//! Stable element keys, for tests, headless scripts and `--tree` inspection.
//!
//! rune-kit keys parts of its components with short names (`label`, `input`, `scroll`, …),
//! and keys share one namespace, so every Krune key carries a recognisable prefix.

pub const NEW_SESSION: &str = "krune-new-session";
pub const SETTINGS: &str = "krune-settings";
pub const BRAND: &str = "krune-brand";
pub const TRANSCRIPT: &str = "krune-transcript";
pub const TRANSCRIPT_SCROLL: &str = "krune-transcript-scroll";
pub const EMPTY_STATE: &str = "krune-empty-state";
pub const COMPOSER: &str = "krune-composer";
pub const COMPOSER_HINT: &str = "krune-composer-hint";
pub const SEND: &str = "krune-send";
pub const STOP: &str = "krune-stop";
pub const STATUS: &str = "krune-agent-status";
pub const RUN_ERROR: &str = "krune-run-error";
pub const CHANGES_TOGGLE: &str = "krune-changes-toggle";
pub const CHANGES: &str = "krune-changes";
pub const SETTINGS_PAGE: &str = "krune-settings-page";
pub const SETTINGS_BACK: &str = "krune-settings-back";
pub const SETTING_DARK: &str = "krune-setting-dark";
pub const SETTING_CHANGES: &str = "krune-setting-changes";

pub fn session(id: krune_core::SessionId) -> String {
    format!("krune-session-{id}")
}

pub fn message(id: krune_core::MessageId) -> String {
    format!("krune-message-{id}")
}

/// Provider and model pickers. rune-kit keys a select's trigger `<key>-trigger` and its
/// options `option<i>` inside `<key>-content`.
pub const COMPOSER_PROVIDER: &str = "krune-composer-provider";
pub const COMPOSER_MODEL: &str = "krune-composer-model";
pub const SETTINGS_PROVIDER: &str = "krune-settings-provider";
pub const SETTINGS_MODEL: &str = "krune-settings-model";
