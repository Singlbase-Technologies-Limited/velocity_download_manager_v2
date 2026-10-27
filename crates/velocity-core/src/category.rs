//! Download categories (#31): the built-in ones from schema 6 and the user's, what a category
//! may hold, and the automatic category for a file name. Stored by `database::categories`.

use serde::{Deserialize, Serialize};

use crate::destination::check_folder;
use crate::domain::ErrorCode;
use crate::naming::extension_of;

/// The longest category name, in characters.
pub const MAX_NAME_CHARS: usize = 40;
/// The most extensions one category lists.
pub const MAX_EXTENSIONS: usize = 100;
/// The longest extension, in characters.
pub const MAX_EXTENSION_CHARS: usize = 16;
/// The icons a category may have; the UI draws each one.
pub const ICONS: [&str; 12] = [
    "folder",
    "archive",
    "file-text",
    "music",
    "film",
    "app-window",
    "disc",
    "image",
    "code",
    "book",
    "package",
    "star",
];

/// One category as the UI sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Category {
    pub id: String,
    pub name: String,
    /// `#rrggbb`, lowercase.
    pub color: String,
    /// One of [`ICONS`].
    pub icon: String,
    /// Lowercase, without the dot, each listed once.
    pub extensions: Vec<String>,
    /// The category's own folder, if it has one.
    pub directory: Option<String>,
    /// A built-in category can be edited but not deleted.
    pub builtin: bool,
}

/// What the user sets on a category, when creating or editing it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CategoryInput {
    pub name: String,
    pub color: String,
    pub icon: String,
    pub extensions: Vec<String>,
    pub directory: Option<String>,
}

impl CategoryInput {
    /// The input as it is stored, or `InvalidInput` (`InvalidPath` for the folder). The name and
    /// folder are trimmed, the colour lowercased, and the extensions lowercased without their
    /// dot, empty ones dropped and each kept once. An empty folder is none.
    pub fn checked(&self) -> Result<CategoryInput, ErrorCode> {
        let name = self.name.trim();
        let name_ok = !name.is_empty()
            && name.chars().count() <= MAX_NAME_CHARS
            && !name.chars().any(char::is_control);
        let color = self.color.trim().to_ascii_lowercase();
        let color_ok = color.len() == 7
            && color.starts_with('#')
            && color[1..].chars().all(|c| c.is_ascii_hexdigit());
        if !name_ok
            || !color_ok
            || !ICONS.contains(&self.icon.as_str())
            || self.extensions.len() > MAX_EXTENSIONS
        {
            return Err(ErrorCode::InvalidInput);
        }
        let mut extensions: Vec<String> = Vec::new();
        for raw in &self.extensions {
            let ext = raw.trim().trim_start_matches('.').to_ascii_lowercase();
            if ext.is_empty() {
                continue;
            }
            if ext.len() > MAX_EXTENSION_CHARS || !ext.chars().all(|c| c.is_ascii_alphanumeric()) {
                return Err(ErrorCode::InvalidInput);
            }
            if !extensions.contains(&ext) {
                extensions.push(ext);
            }
        }
        let directory = match self.directory.as_deref().map(str::trim) {
            Some(d) if !d.is_empty() => Some(check_folder(d)?.to_string_lossy().into_owned()),
            _ => None,
        };
        Ok(CategoryInput {
            name: name.to_owned(),
            color,
            icon: self.icon.clone(),
            extensions,
            directory,
        })
    }
}

/// The automatic category for `file_name`: the first of `categories` (in their order) that lists
/// its extension.
pub fn for_file_name<'a>(categories: &'a [Category], file_name: &str) -> Option<&'a Category> {
    let ext = extension_of(file_name)?;
    categories.iter().find(|c| c.extensions.contains(&ext))
}

impl Category {
    /// A fixed example for the contract fixtures and UI tests.
    pub fn sample() -> Self {
        Self {
            id: "c-0a1b2c3d4e5f".to_owned(),
            name: "Курсы".to_owned(),
            color: "#112233".to_owned(),
            icon: "book".to_owned(),
            extensions: vec!["mp4".to_owned(), "pdf".to_owned()],
            directory: Some("/home/user/Courses".to_owned()),
            builtin: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(name: &str) -> CategoryInput {
        CategoryInput {
            name: name.to_owned(),
            color: "#112233".to_owned(),
            icon: "folder".to_owned(),
            extensions: vec![".ABC".to_owned(), "abc".to_owned(), " ".to_owned()],
            directory: None,
        }
    }

    fn category(id: &str, extensions: &[&str]) -> Category {
        Category {
            id: id.to_owned(),
            name: id.to_owned(),
            color: "#112233".to_owned(),
            icon: "folder".to_owned(),
            extensions: extensions.iter().map(|e| (*e).to_owned()).collect(),
            directory: None,
            builtin: false,
        }
    }

    #[test]
    fn an_input_is_tidied_as_it_is_stored() {
        let tidy = CategoryInput {
            name: "  Mine ".to_owned(),
            color: "#AABBCC".to_owned(),
            directory: Some("  ".to_owned()),
            ..input("x")
        }
        .checked()
        .unwrap();
        assert_eq!(tidy.name, "Mine");
        assert_eq!(tidy.color, "#aabbcc");
        assert_eq!(tidy.extensions, vec!["abc"]);
        assert_eq!(tidy.directory, None);
        let name = "é".repeat(MAX_NAME_CHARS);
        assert_eq!(input(&name).checked().unwrap().name, name);
    }

    #[test]
    fn an_input_v1_refused_is_refused() {
        let invalid = Err(ErrorCode::InvalidInput);
        let with = |f: fn(&mut CategoryInput)| {
            let mut i = input("A");
            f(&mut i);
            i.checked()
        };
        assert_eq!(with(|i| i.name = " ".to_owned()), invalid);
        assert_eq!(with(|i| i.name = "x".repeat(MAX_NAME_CHARS + 1)), invalid);
        assert_eq!(with(|i| i.name = "a\nb".to_owned()), invalid);
        assert_eq!(with(|i| i.color = "red".to_owned()), invalid);
        assert_eq!(with(|i| i.color = "#12345g".to_owned()), invalid);
        assert_eq!(with(|i| i.icon = "<script>".to_owned()), invalid);
        assert_eq!(with(|i| i.extensions = vec!["../x".to_owned()]), invalid);
        assert_eq!(with(|i| i.extensions = vec!["x".repeat(17)]), invalid);
        assert_eq!(
            with(|i| i.extensions = (0..=MAX_EXTENSIONS).map(|n| format!("e{n}")).collect()),
            invalid
        );
        assert_eq!(
            with(|i| i.directory = Some("relative".to_owned())),
            Err(ErrorCode::InvalidPath)
        );
    }

    #[test]
    fn the_first_category_listing_the_extension_is_the_automatic_one() {
        let categories = [
            category("video", &["mp4", "ts"]),
            category("mine", &["mp4", "abc"]),
        ];
        let id = |name| for_file_name(&categories, name).map(|c| c.id.as_str());
        assert_eq!(id("movie.MP4"), Some("video"));
        assert_eq!(id("x.abc"), Some("mine"));
        assert_eq!(id("archive.tar.ts"), Some("video"));
        assert_eq!(id("README"), None);
        assert_eq!(id(".abc"), None);
        assert_eq!(id("x.zip"), None);
    }
}
