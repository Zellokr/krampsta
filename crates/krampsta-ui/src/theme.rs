//! Krampsta's look: OpenCode's design language (`rune-theme-opencode`), the same theme Rugents uses —
//! near-black neutral surfaces, white type, a blue focus ring, Inter and IBM Plex Mono.
//!
//! Components read only the `Kit`'s semantic tokens, so this file is the one place the theme is
//! chosen. Swapping it means changing [`kit`], [`FONTS`] and [`FAMILIES`].

use rune::Color;
use rune_kit::Kit;

/// Faces the theme is drawn with (Inter, IBM Plex Mono), for `AppSpec::fonts`.
pub const FONTS: &[&[u8]] = rune_theme_opencode::FONTS;
/// Sans and mono family names, for `AppSpec::families`.
pub const FAMILIES: (&str, &str) = rune_theme_opencode::FAMILIES;

/// How long a streamed word takes to fade in (seconds): Rune's own streaming value.
pub const WORD_FADE: f32 = 0.18;

/// Which appearance the user picked. Dark is the default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeChoice {
    #[default]
    Dark,
    Light,
    /// Follow the operating system.
    System,
}

impl ThemeChoice {
    pub fn is_dark(self, system_dark: bool) -> bool {
        match self {
            ThemeChoice::Dark => true,
            ThemeChoice::Light => false,
            ThemeChoice::System => system_dark,
        }
    }
}

/// The kit every view builds with (built once per appearance and blend mode, then cached).
pub fn kit(dark: bool) -> &'static Kit {
    rune_theme_opencode::kit(dark)
}

/// The colour for Krampsta's mark and "working" spinners. The theme is neutral, so it is the
/// strongest icon ink rather than a brand hue.
pub fn accent(dark: bool) -> Color {
    kit(dark).c.icon_base
}

/// Colour of a status dot.
pub fn status_color(k: &Kit, status: &krampsta_core::AgentStatus) -> Color {
    use krampsta_core::AgentStatus as S;
    match status {
        S::Idle => k.c.icon_faint,
        S::Running => k.c.icon_base,
        S::Completed => k.c.success.icon,
        S::Failed { .. } => k.c.danger.icon,
        S::Cancelled => k.c.warning.icon,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_is_the_default_choice() {
        assert!(ThemeChoice::default().is_dark(false));
        assert!(!ThemeChoice::Light.is_dark(true));
        assert!(ThemeChoice::System.is_dark(true));
    }

    #[test]
    fn kits_follow_the_appearance() {
        assert!(kit(true).dark);
        assert!(!kit(false).dark);
        assert!(kit(true).c.text_base.luminance() > kit(true).c.bg_base.luminance());
    }
}
