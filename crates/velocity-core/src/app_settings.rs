//! The app's own settings (#29), V1's `general`, `notifications` and `appearance` settings and its
//! first-run flag. Each is a section of the settings document ([`Section`]); #30 puts them on
//! the Settings screen.

use std::collections::BTreeMap;

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

/// A column of the download list (#59, V1's column ids).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ColumnId {
    Name,
    Size,
    Downloaded,
    Progress,
    Speed,
    AvgSpeed,
    Eta,
    Status,
    Category,
    Destination,
    Added,
    Started,
    Completed,
}

impl ColumnId {
    /// Every column, in the order the column chooser lists them.
    pub const ALL: [Self; 13] = [
        Self::Name,
        Self::Size,
        Self::Downloaded,
        Self::Progress,
        Self::Speed,
        Self::AvgSpeed,
        Self::Eta,
        Self::Status,
        Self::Category,
        Self::Destination,
        Self::Added,
        Self::Started,
        Self::Completed,
    ];

    /// The columns of a new install (V1's).
    pub const DEFAULT: [Self; 8] = [
        Self::Name,
        Self::Size,
        Self::Progress,
        Self::Speed,
        Self::Eta,
        Self::Status,
        Self::Category,
        Self::Added,
    ];
}

/// Bounds for a column width the user set, in CSS pixels (V1's). Mirrored in the UI.
pub const COLUMN_MIN_WIDTH: u32 = 48;
pub const COLUMN_MAX_WIDTH: u32 = 1200;

/// How the app looks (V1's `appearance` settings).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct AppearanceSettings {
    pub theme: Theme,
    pub density: Density,
    pub layout: LayoutSettings,
    /// The download list's columns, in display order (#59).
    pub visible_columns: Vec<ColumnId>,
    /// The widths the user set, in CSS pixels; a column not listed has its default width (#59).
    pub column_widths: BTreeMap<ColumnId, u32>,
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        AppearanceSettings {
            theme: Theme::default(),
            density: Density::default(),
            layout: LayoutSettings::default(),
            visible_columns: ColumnId::DEFAULT.to_vec(),
            column_widths: BTreeMap::new(),
        }
    }
}

impl Section for AppearanceSettings {
    const KEY: &'static str = "appearance";

    /// `InvalidInput` for a panel or column width outside its bounds, and for a column list
    /// without the Name column or with a column twice.
    fn checked(&self) -> Result<Self, ErrorCode> {
        let within =
            |width: Option<u32>, min: u32, max: u32| width.is_none_or(|w| (min..=max).contains(&w));
        let layout = &self.layout;
        let columns = &self.visible_columns;
        let once = columns
            .iter()
            .enumerate()
            .all(|(i, c)| !columns[..i].contains(c));
        if within(layout.sidebar_width, SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH)
            && within(layout.details_width, DETAILS_MIN_WIDTH, DETAILS_MAX_WIDTH)
            && columns.contains(&ColumnId::Name)
            && once
            && self
                .column_widths
                .values()
                .all(|w| (COLUMN_MIN_WIDTH..=COLUMN_MAX_WIDTH).contains(w))
        {
            Ok(self.clone())
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
                ..AppearanceSettings::default()
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
    fn the_list_starts_with_v1s_columns_at_their_default_widths() {
        use ColumnId::*;
        let look = AppearanceSettings::default();
        assert_eq!(
            look.visible_columns,
            [Name, Size, Progress, Speed, Eta, Status, Category, Added]
        );
        assert!(look.column_widths.is_empty());
    }

    #[test]
    fn the_column_list_keeps_the_name_and_each_column_once() {
        use ColumnId::*;
        let with = |columns: Vec<ColumnId>| AppearanceSettings {
            visible_columns: columns,
            ..AppearanceSettings::default()
        };
        for ok in [with(vec![Name]), with(ColumnId::ALL.to_vec())] {
            assert_eq!(ok.checked(), Ok(ok.clone()));
        }
        for bad in [with(vec![]), with(vec![Size]), with(vec![Name, Size, Size])] {
            assert_eq!(bad.checked(), Err(ErrorCode::InvalidInput), "{bad:?}");
        }
    }

    #[test]
    fn column_widths_must_be_within_their_bounds() {
        let with = |width: u32| AppearanceSettings {
            column_widths: BTreeMap::from([(ColumnId::Destination, width)]),
            ..AppearanceSettings::default()
        };
        for ok in [with(COLUMN_MIN_WIDTH), with(COLUMN_MAX_WIDTH)] {
            assert_eq!(ok.checked(), Ok(ok.clone()));
        }
        for bad in [with(COLUMN_MIN_WIDTH - 1), with(COLUMN_MAX_WIDTH + 1)] {
            assert_eq!(bad.checked(), Err(ErrorCode::InvalidInput), "{bad:?}");
        }
    }

    #[test]
    fn saved_columns_are_read_by_v1s_ids_and_an_unknown_one_is_refused() {
        let look: AppearanceSettings = serde_json::from_value(serde_json::json!({
            "visibleColumns": ["name", "avgSpeed", "started"],
            "columnWidths": { "avgSpeed": 120 }
        }))
        .unwrap();
        assert_eq!(
            look.visible_columns,
            [ColumnId::Name, ColumnId::AvgSpeed, ColumnId::Started]
        );
        assert_eq!(look.column_widths[&ColumnId::AvgSpeed], 120);
        let unknown = serde_json::from_value::<AppearanceSettings>(serde_json::json!({
            "visibleColumns": ["name", "gid"]
        }));
        assert!(unknown.is_err());
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
