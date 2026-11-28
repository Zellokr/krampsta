//! Stable element keys, for tests, headless scripts and `--tree` inspection.
//!
//! rune-kit keys parts of its components with short names (`label`, `input`, `scroll`, …),
//! and keys share one namespace, so every Krampsta key carries a recognisable prefix.

pub const NEW_SESSION: &str = "krampsta-new-session";
/// The project folder row; pressing it opens the folder dialog.
pub const PROJECT: &str = "krampsta-project";
pub const SETTINGS: &str = "krampsta-settings";
pub const BRAND: &str = "krampsta-brand";
pub const TRANSCRIPT: &str = "krampsta-transcript";
pub const TRANSCRIPT_SCROLL: &str = "krampsta-transcript-scroll";
pub const EMPTY_STATE: &str = "krampsta-empty-state";
pub const COMPOSER: &str = "krampsta-composer";
/// The rewind confirmation and its buttons.
pub const REWIND: &str = "krampsta-rewind";
pub const REWIND_CONFIRM: &str = "krampsta-rewind-confirm";
pub const REWIND_CANCEL: &str = "krampsta-rewind-cancel";
/// The composer's yolo-mode switch.
pub const COMPOSER_YOLO: &str = "krampsta-composer-yolo";
pub const COMPOSER_HINT: &str = "krampsta-composer-hint";
/// The image attached to the draft, and its remove button.
pub const COMPOSER_IMAGE: &str = "krampsta-composer-image";
pub const COMPOSER_IMAGE_REMOVE: &str = "krampsta-composer-image-remove";
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

/// A running call's live output (a command's last lines).
/// The change a call proposed, shown in its card.
/// The image a user message carries.
pub fn message_image(id: krampsta_core::MessageId) -> String {
    format!("krampsta-message-image-{id}")
}

pub fn tool_diff(id: &krampsta_core::ToolCallId) -> String {
    format!("krampsta-tool-diff-{id}")
}

pub fn tool_live(id: &krampsta_core::ToolCallId) -> String {
    format!("krampsta-tool-live-{id}")
}

/// The dialog that asks to allow a command (rune-kit keys its panel `<key>-dialog`), and its
/// buttons.
pub const APPROVAL: &str = "krampsta-approval";
pub const APPROVAL_COMMAND: &str = "krampsta-approval-command";
pub const APPROVAL_DENY: &str = "krampsta-approval-deny";
pub const APPROVAL_ONCE: &str = "krampsta-approval-once";
pub const APPROVAL_ALWAYS: &str = "krampsta-approval-always";
pub const APPROVAL_APPLY_AND_VALIDATE: &str = "krampsta-approval-apply-and-validate";

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

/// A provider's API key field in settings, and its Save and Remove buttons.
pub fn api_key(provider: &str) -> String {
    format!("krampsta-api-key-{provider}")
}

pub fn api_key_save(provider: &str) -> String {
    format!("krampsta-api-key-save-{provider}")
}

pub fn api_key_remove(provider: &str) -> String {
    format!("krampsta-api-key-remove-{provider}")
}

/// The changes panel's actions: a whole file's (in its header), a block's (under it), and why a
/// write failed.
pub fn change_accept(file: usize) -> String {
    format!("krampsta-change-accept-{file}")
}

pub fn change_reject(file: usize) -> String {
    format!("krampsta-change-reject-{file}")
}

pub fn block_accept(file: usize, block: usize) -> String {
    format!("krampsta-block-accept-{file}-{block}")
}

pub fn block_reject(file: usize, block: usize) -> String {
    format!("krampsta-block-reject-{file}-{block}")
}

pub const CHANGES_ERROR: &str = "krampsta-changes-error";

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

/// The steps of a plan the agent keeps (`update_plan`), in its call's card.
pub fn plan(call: &krampsta_core::ToolCallId) -> String {
    format!("krampsta-plan-{call}")
}

/// The header of a summary of earlier messages, which opens it.
pub fn summary(id: krampsta_core::MessageId) -> String {
    format!("krampsta-summary-{id}")
}

/// A project's group of sessions in the sidebar (None: "No project"). Keyed by the folder's
/// whole path, so two projects with the same name never share a key.
pub fn project_group(root: Option<&std::path::Path>) -> String {
    use std::hash::{Hash, Hasher};
    let Some(root) = root else { return "krampsta-group-none".to_owned() };
    let mut hasher = std::hash::DefaultHasher::new();
    root.hash(&mut hasher);
    format!("krampsta-group-{:016x}", hasher.finish())
}

/// The button on another project's group that opens that folder.
pub fn open_group(root: Option<&std::path::Path>) -> String {
    format!("{}-open", project_group(root))
}

/// A billed model's price field in settings; `side` is "input" or "output".
pub fn price(provider: &str, model: &str, side: &str) -> String {
    format!("krampsta-price-{provider}-{model}-{side}")
}

/// The strip of open sessions above the conversation.
pub const TABS: &str = "krampsta-tabs";

/// A session's id in the tab strip; rune-kit keys the tab itself `t-<this>`.
pub fn tab(id: krampsta_core::SessionId) -> String {
    format!("krampsta-tab-{}", id.0)
}

/// The session a tab id ([`tab`]) names.
pub fn tab_session(tab: &str) -> Option<krampsta_core::SessionId> {
    tab.strip_prefix("krampsta-tab-")?.parse().ok().map(krampsta_core::SessionId)
}

/// The two conversation panes side by side, and each pane (0 left, 1 right). The left (or only)
/// pane's parts keep their usual keys; a path reaches the right one's (`krampsta-pane-1/…`).
pub const PANES: &str = "krampsta-panes";
pub fn pane(index: usize) -> String {
    format!("krampsta-pane-{index}")
}
pub fn pane_header(index: usize) -> String {
    format!("krampsta-pane-{index}-header")
}
pub fn pane_close(index: usize) -> String {
    format!("krampsta-pane-{index}-close")
}
/// The toolbar button that opens or closes the split.
pub const SPLIT_TOGGLE: &str = "krampsta-split-toggle";

/// The MCP servers in Settings: one card each, its state, and the card shown when there are none.
pub const MCP_SERVERS_EMPTY: &str = "krampsta-mcp-servers-empty";

pub fn mcp_server(name: &str) -> String {
    format!("krampsta-mcp-server-{name}")
}

pub fn mcp_server_state(name: &str) -> String {
    format!("krampsta-mcp-server-state-{name}")
}

/// A server's switch, its Remove button and the second click that confirms it.
pub fn mcp_server_toggle(name: &str) -> String {
    format!("krampsta-mcp-server-toggle-{name}")
}

pub fn mcp_server_remove(name: &str) -> String {
    format!("krampsta-mcp-server-remove-{name}")
}

pub fn mcp_server_remove_confirm(name: &str) -> String {
    format!("krampsta-mcp-server-remove-confirm-{name}")
}

/// The "Add a server" button and the form it opens.
pub const MCP_ADD_OPEN: &str = "krampsta-mcp-add-open";
pub const MCP_ADD_FORM: &str = "krampsta-mcp-add-form";
pub const MCP_ADD_KIND: &str = "krampsta-mcp-add-kind";
pub const MCP_ADD_NAME: &str = "krampsta-mcp-add-name";
pub const MCP_ADD_TARGET: &str = "krampsta-mcp-add-target";
pub const MCP_ADD_EXTRA: &str = "krampsta-mcp-add-extra";
pub const MCP_ADD_SAVE: &str = "krampsta-mcp-add-save";
pub const MCP_ADD_ERROR: &str = "krampsta-mcp-add-error";

/// The right panel's switch between "Changes" and "Files", and the "Files" view.
pub const INSPECTOR_TABS: &str = "krampsta-inspector-tabs";
pub const FILES_TREE: &str = "krampsta-files-tree";
pub const FILES_FILTER: &str = "krampsta-files-filter";
pub const FILES_REFRESH: &str = "krampsta-files-refresh";
pub const FILES_EMPTY: &str = "krampsta-files-empty";

/// The "Git" view: the diff, its refresh button, the empty or failed state, the skipped files.
pub const GIT_DIFF: &str = "krampsta-git-diff";
pub const GIT_REFRESH: &str = "krampsta-git-refresh";
pub const GIT_EMPTY: &str = "krampsta-git-empty";
pub const GIT_SKIPPED: &str = "krampsta-git-skipped";

/// The sidebar's session search, and the line it shows when nothing matches.
pub const SESSION_SEARCH: &str = "krampsta-session-search";
pub const SESSION_SEARCH_EMPTY: &str = "krampsta-session-search-empty";

/// The command palette: its dialog, field, rows and the line shown when nothing matches.
pub const PALETTE: &str = "krampsta-palette";
pub const PALETTE_INPUT: &str = "krampsta-palette-input";
pub const PALETTE_EMPTY: &str = "krampsta-palette-empty";

pub fn palette_item(index: usize) -> String {
    format!("krampsta-palette-item-{index}")
}

/// A code block's Copy button, in a message.
pub fn code_copy(message: krampsta_core::MessageId, index: usize) -> String {
    format!("krampsta-code-copy-{message}-{index}")
}
