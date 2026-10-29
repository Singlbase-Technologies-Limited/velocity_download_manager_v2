//! The app's own settings (#29), V1's `general`, `notifications` and `appearance` settings and its
//! first-run flag. Each is a section of the settings document ([`Section`]); #30 puts them on
//! the Settings screen.

use serde::{Deserialize, Serialize};

use crate::database::settings::Section;
use crate::domain::ErrorCode;

/// What closing the window and starting the app do (V1's `general` settings).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct GeneralSettings {
    /// Closing the window hides it in the tray, when there is one, instead of quitting.
    pub close_to_tray: bool,
    /// Start hidden in the tray, when there is one.
    pub start_minimized: bool,
    /// Ask before quitting while downloads are active.
    pub confirm_exit_with_active: bool,
}

impl Default for GeneralSettings {
    /// V1's defaults.
    fn default() -> Self {
        GeneralSettings {
            close_to_tray: true,
            start_minimized: false,
            confirm_exit_with_active: true,
        }
    }
}

impl Section for GeneralSettings {
    const KEY: &'static str = "general";
}

/// Which desktop notifications are shown (V1's `notifications` settings).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct NotificationSettings {
    /// A download completed, or a torrent waits for its files to be chosen.
    pub on_complete: bool,
    pub on_failure: bool,
    pub on_start: bool,
    /// The transfer engine or the torrent helper stopped and won't start again on its own.
    pub on_engine_failure: bool,
}

impl Default for NotificationSettings {
    /// V1's defaults: everything but a download starting.
    fn default() -> Self {
        NotificationSettings {
            on_complete: true,
            on_failure: true,
            on_start: false,
            on_engine_failure: true,
        }
    }
}

impl Section for NotificationSettings {
    const KEY: &'static str = "notifications";
}

/// The colour theme: the system's, or always light or dark.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

impl Theme {
    /// Every theme, in the order the UI offers them.
    pub const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];
}

/// How tightly lists are laid out.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Density {
    #[default]
    Compact,
    Comfortable,
}

impl Density {
    /// Every density, in the order the UI offers them.
    pub const ALL: [Self; 2] = [Self::Compact, Self::Comfortable];
}

/// Bounds for saved panel widths, in CSS pixels, V1's. Mirrored in `src/layout/layoutLogic.ts`.
pub const SIDEBAR_MIN_WIDTH: u32 = 184;
pub const SIDEBAR_MAX_WIDTH: u32 = 360;
pub const DETAILS_MIN_WIDTH: u32 = 280;
pub const DETAILS_MAX_WIDTH: u32 = 600;

/// The window's panels (V1's `appearance.layout`). Purely visual: it never affects a download.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct LayoutSettings {
    /// The navigation's width; `None` for the responsive default.
    pub sidebar_width: Option<u32>,
    /// The navigation shows icons only.
    pub sidebar_collapsed: bool,
    pub sidebar_visible: bool,
    /// The details panel's width; `None` for the responsive default.
    pub details_width: Option<u32>,
    pub details_visible: bool,
}

impl Default for LayoutSettings {
    fn default() -> Self {
        LayoutSettings {
            sidebar_width: None,
            sidebar_collapsed: false,
            sidebar_visible: true,
            details_width: None,
            details_visible: false,
        }
    }
}

/// How the app looks (V1's `appearance` settings, without the download list's columns, which are
/// #59's).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct AppearanceSettings {
    pub theme: Theme,
    pub density: Density,
    pub layout: LayoutSettings,
}

impl Section for AppearanceSettings {
    const KEY: &'static str = "appearance";

    /// `InvalidInput` for a panel width outside its bounds.
    fn checked(&self) -> Result<Self, ErrorCode> {
        let within =
            |width: Option<u32>, min: u32, max: u32| width.is_none_or(|w| (min..=max).contains(&w));
        let layout = &self.layout;
        if within(layout.sidebar_width, SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH)
            && within(layout.details_width, DETAILS_MIN_WIDTH, DETAILS_MAX_WIDTH)
        {
            Ok(*self)
        } else {
            Err(ErrorCode::InvalidInput)
        }
    }
}

/// Whether the first-run setup was finished (V1's `onboardingCompleted`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct OnboardingSettings {
    pub completed: bool,
}

impl Section for OnboardingSettings {
    const KEY: &'static str = "onboarding";
}

/// The settings the window starts with (#29): how it looks, whether the first-run setup is
/// needed, and the folder downloads go to now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellSettings {
    pub appearance: AppearanceSettings,
    pub onboarding_completed: bool,
    /// The default folder as it applies now: the setting, else the system's Downloads folder;
    /// `None` when there is neither.
    pub default_directory: Option<String>,
}

impl ShellSettings {
    /// A fixed value for the contract fixture.
    pub fn sample() -> Self {
        ShellSettings {
            appearance: AppearanceSettings {
                theme: Theme::Dark,
                density: Density::Comfortable,
                layout: LayoutSettings {
                    sidebar_width: Some(240),
                    sidebar_collapsed: false,
                    sidebar_visible: true,
                    details_width: None,
                    details_visible: true,
                },
            },
            onboarding_completed: true,
            default_directory: Some("/home/user/Downloads".to_owned()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_v1s() {
        let general = GeneralSettings::default();
        assert!(general.close_to_tray);
        assert!(!general.start_minimized);
        assert!(general.confirm_exit_with_active);
        let notify = NotificationSettings::default();
        assert!(notify.on_complete && notify.on_failure && notify.on_engine_failure);
        assert!(!notify.on_start);
        let look = AppearanceSettings::default();
        assert_eq!(
            (look.theme, look.density),
            (Theme::System, Density::Compact)
        );
        assert_eq!(look.layout.sidebar_width, None);
        assert!(look.layout.sidebar_visible && !look.layout.sidebar_collapsed);
        assert!(!look.layout.details_visible);
        assert!(!OnboardingSettings::default().completed);
    }

    #[test]
    fn panel_widths_must_be_within_their_bounds() {
        let with = |sidebar: Option<u32>, details: Option<u32>| AppearanceSettings {
            layout: LayoutSettings {
                sidebar_width: sidebar,
                details_width: details,
                ..LayoutSettings::default()
            },
            ..AppearanceSettings::default()
        };
        for ok in [
            with(None, None),
            with(Some(SIDEBAR_MIN_WIDTH), Some(DETAILS_MAX_WIDTH)),
            with(Some(SIDEBAR_MAX_WIDTH), Some(DETAILS_MIN_WIDTH)),
        ] {
            assert_eq!(ok.checked(), Ok(ok));
        }
        for bad in [
            with(Some(SIDEBAR_MIN_WIDTH - 1), None),
            with(Some(SIDEBAR_MAX_WIDTH + 1), None),
            with(None, Some(DETAILS_MIN_WIDTH - 1)),
            with(None, Some(DETAILS_MAX_WIDTH + 1)),
        ] {
            assert_eq!(bad.checked(), Err(ErrorCode::InvalidInput), "{bad:?}");
        }
    }

    #[test]
    fn saved_settings_are_read_in_camel_case() {
        let look: AppearanceSettings = serde_json::from_value(serde_json::json!({
            "theme": "dark",
            "layout": { "sidebarWidth": 200, "detailsVisible": true }
        }))
        .unwrap();
        assert_eq!(look.theme, Theme::Dark);
        assert_eq!(look.density, Density::Compact, "missing: the default");
        assert_eq!(look.layout.sidebar_width, Some(200));
        assert!(look.layout.details_visible && look.layout.sidebar_visible);
    }
}
