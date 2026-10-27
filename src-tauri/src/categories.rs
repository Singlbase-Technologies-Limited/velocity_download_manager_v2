//! Categories and destinations (#31): V1's category commands, a download's category, and what
//! adding a download would do. Commands stay thin: the manager checks every value.

use tauri::State;
use velocity_core::category::{Category, CategoryInput};
use velocity_core::destination::Choice;
use velocity_core::domain::{DestinationCheck, ErrorCode};

use crate::downloads::{on_manager, Downloads};

#[tauri::command]
pub async fn list_categories(downloads: State<'_, Downloads>) -> Result<Vec<Category>, ErrorCode> {
    on_manager(&downloads, |m| m.categories()).await?
}

#[tauri::command]
pub async fn create_category(
    downloads: State<'_, Downloads>,
    input: CategoryInput,
) -> Result<Category, ErrorCode> {
    on_manager(&downloads, move |m| m.create_category(&input)).await?
}

#[tauri::command]
pub async fn update_category(
    downloads: State<'_, Downloads>,
    id: String,
    input: CategoryInput,
) -> Result<Category, ErrorCode> {
    on_manager(&downloads, move |m| m.update_category(&id, &input)).await?
}

/// Deletes a user category; its downloads are left without one, and no file is touched.
#[tauri::command]
pub async fn delete_category(downloads: State<'_, Downloads>, id: String) -> Result<(), ErrorCode> {
    on_manager(&downloads, move |m| m.delete_category(&id)).await?
}

/// Sets or clears (`null`) a download's category. Nothing is moved.
#[tauri::command]
pub async fn set_download_category(
    downloads: State<'_, Downloads>,
    id: String,
    category_id: Option<String>,
) -> Result<(), ErrorCode> {
    on_manager(&downloads, move |m| {
        m.set_download_category(&id, category_id.as_deref())
    })
    .await?
}

/// What adding `url` would do with these choices, each `null` for automatic: the folder, the
/// category, the file's name and whether it is taken.
#[tauri::command]
pub async fn check_destination(
    downloads: State<'_, Downloads>,
    url: String,
    directory: Option<String>,
    file_name: Option<String>,
    category_id: Option<String>,
) -> Result<DestinationCheck, ErrorCode> {
    let choice = Choice {
        directory,
        file_name,
        category_id,
    };
    on_manager(&downloads, move |m| m.check_destination(&url, &choice)).await?
}
