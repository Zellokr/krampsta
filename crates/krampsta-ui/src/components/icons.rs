//! Icons: the theme's own catalog (OpenCode's, as Rugents uses it) plus Krampsta's mark.

use rune::Icon;

pub use rune_theme_opencode::icons::{ARROW_UP, PLUS, SETTINGS_GEAR as GEAR, STOP, TERMINAL as TOOL};

/// The Krampsta mark: a rune-like K inside a rounded square (16-point SVG, current colour).
pub static KRAMPSTA: Icon = Icon {
    name: "krampsta-mark",
    view: 16.0,
    body: r#"<rect x="1.5" y="1.5" width="13" height="13" rx="3.5" stroke="currentColor" stroke-width="1.3" fill="none"/><path d="M6 4.5V11.5M6 8L10 4.5M7.3 7L10 11.5" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};
