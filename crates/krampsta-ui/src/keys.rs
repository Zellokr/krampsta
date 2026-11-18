//! Stable element keys, for tests, headless scripts and `--tree` inspection.
//!
//! rune-kit keys parts of its components with short names (`label`, `input`, `scroll`, …),
//! and keys share one namespace, so every Krampsta key carries a recognisable prefix.

pub const NEW_SESSION: &str = "krampsta-new-session";
pub const SETTINGS: &str = "krampsta-settings";
pub const BRAND: &str = "krampsta-brand";
pub const TRANSCRIPT: &str = "krampsta-transcript";
pub const TRANSCRIPT_SCROLL: &str = "krampsta-transcript-scroll";
pub const EMPTY_STATE: &str = "krampsta-empty-state";
pub const COMPOSER: &str = "krampsta-composer";
pub const COMPOSER_HINT: &str = "krampsta-composer-hint";
pub const SEND: &str = "krampsta-send";
pub const STOP: &str = "krampsta-stop";
pub const STATUS: &str = "krampsta-agent-status";
pub const RUN_ERROR: &str = "krampsta-run-error";
/// The shimmering "working" lines: before the reply starts, and while it has no text yet.
pub const STARTING: &str = "krampsta-starting";
pub const THINKING: &str = "krampsta-thinking";
pub const CHANGES_TOGGLE: &str = "krampsta-changes-toggle";
pub const CHANGES: &str = "krampsta-changes";
pub const CHANGES_EMPTY: &str = "krampsta-changes-empty";
pub const SETTINGS_PAGE: &str = "krampsta-settings-page";
pub const SETTINGS_SCROLL: &str = "krampsta-settings-scroll";
pub const SETTINGS_BACK: &str = "krampsta-settings-back";
pub const SETTING_DARK: &str = "krampsta-setting-dark";
pub const SETTING_CHANGES: &str = "krampsta-setting-changes";

pub fn session(id: krampsta_core::SessionId) -> String {
    format!("krampsta-session-{id}")
}

pub fn message(id: krampsta_core::MessageId) -> String {
    format!("krampsta-message-{id}")
}

/// A tool card's header (the button that opens and closes it).
pub fn tool(id: &krampsta_core::ToolCallId) -> String {
    format!("krampsta-tool-{id}")
}

/// Provider and model pickers. rune-kit keys a select's trigger `<key>-trigger` and its
/// options `option<i>` inside `<key>-content`.
pub const COMPOSER_PROVIDER: &str = "krampsta-composer-provider";
pub const COMPOSER_MODEL: &str = "krampsta-composer-model";
pub const SETTINGS_PROVIDER: &str = "krampsta-settings-provider";
pub const SETTINGS_MODEL: &str = "krampsta-settings-model";

/// The sign-in dialog and its parts.
pub const SIGN_IN: &str = "krampsta-sign-in";
pub const SIGN_IN_CODE: &str = "krampsta-sign-in-code";
pub const SIGN_IN_OPEN: &str = "krampsta-sign-in-open";
pub const SIGN_IN_RETRY: &str = "krampsta-sign-in-retry";

/// The sign-in (or sign-out) button of an account provider in settings.
pub fn account(provider: &str) -> String {
    format!("krampsta-account-{provider}")
}
