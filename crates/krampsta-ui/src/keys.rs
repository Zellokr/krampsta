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
/// The session's token count in the toolbar.
pub const USAGE: &str = "krampsta-usage";
pub const RUN_ERROR: &str = "krampsta-run-error";
/// The shimmering "working" lines: before the reply starts, and while it has no text yet.
pub const STARTING: &str = "krampsta-starting";
pub const WORKING: &str = "krampsta-working";
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
pub const COMPOSER_EFFORT: &str = "krampsta-composer-effort";
pub const SETTINGS_EFFORT: &str = "krampsta-settings-effort";

/// The sign-in dialog and its parts.
pub const SIGN_IN: &str = "krampsta-sign-in";
pub const SIGN_IN_CODE: &str = "krampsta-sign-in-code";
pub const SIGN_IN_OPEN: &str = "krampsta-sign-in-open";
pub const SIGN_IN_RETRY: &str = "krampsta-sign-in-retry";

/// The sign-in (or sign-out) button of an account provider in settings.
pub fn account(provider: &str) -> String {
    format!("krampsta-account-{provider}")
}

/// A session row's context menu; rune-kit keys its items `<key>-item<i>`.
pub fn session_menu(id: krampsta_core::SessionId) -> String {
    format!("krampsta-session-menu-{id}")
}

/// A message's context menu.
pub fn message_menu(id: krampsta_core::MessageId) -> String {
    format!("krampsta-message-menu-{id}")
}

/// The dialog that confirms deleting a session (rune-kit keys its panel `<key>-dialog`), and its
/// buttons.
pub const DELETE_SESSION: &str = "krampsta-delete-session";
pub const DELETE_SESSION_CONFIRM: &str = "krampsta-delete-session-confirm";
pub const DELETE_SESSION_CANCEL: &str = "krampsta-delete-session-cancel";

/// A message's reasoning header (the button that opens and closes it).
pub fn reasoning(id: krampsta_core::MessageId) -> String {
    format!("krampsta-reasoning-{id}")
}
