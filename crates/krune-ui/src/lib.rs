//! Krune's desktop interface, built with Rune and rune-kit.
//!
//! - [`state`]: [`KruneApp`], the whole UI state, and the operations handlers call.
//! - [`app`]: the Rune wiring — [`spec`], the root view, `poll`, keys and headless scenes.
//! - [`views`]: screens and panes (sidebar, chat, changes, settings).
//! - [`picker`]: choosing the provider and model (state and handlers).
//! - [`components`]: small reusable pieces (message bubbles, status pill, composer).
//! - [`theme`]: Krune's palette as a rune-kit `Kit`.
//!
//! Views are pure functions of [`KruneApp`]: no I/O, no blocking, no work beyond building
//! elements. Agent events arrive in `poll` (woken by the agent host's notifier).

pub mod app;
pub mod components;
pub mod keys;
pub mod picker;
pub mod sample;
pub mod state;
pub mod theme;
pub mod views;

pub use app::chrome;
pub use app::spec;
pub use state::KruneApp;
pub use state::Screen;
pub use state::SendError;
pub use theme::ThemeChoice;
