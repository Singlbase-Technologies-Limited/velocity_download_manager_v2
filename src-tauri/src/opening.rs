//! "Open file" and "Open folder" (#61): the manager says what to open, and the system's own
//! program opens it, started with an argument list and never through a shell (PO decision on #61).

use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

use tauri::State;
use velocity_core::domain::ErrorCode;
use velocity_core::manager::FolderView;

use crate::downloads::{on_manager, Downloads};

/// Opens the finished download's file, or a torrent's folder. `CONFIRMATION_REQUIRED` for a
/// program until `allow_program`, which the window sends once the user confirmed.
#[tauri::command]
pub async fn open_download_file(
    downloads: State<'_, Downloads>,
    id: String,
    allow_program: bool,
) -> Result<(), ErrorCode> {
    let path = on_manager(&downloads, move |m| m.file_to_open(&id, allow_program)).await??;
    run(opener(Target::Open(&path)))
}

/// Shows the download's file selected in its folder, or the folder when the file is gone.
#[tauri::command]
pub async fn open_download_folder(
    downloads: State<'_, Downloads>,
    id: String,
) -> Result<(), ErrorCode> {
    let view = on_manager(&downloads, move |m| m.folder_to_open(&id)).await??;
    run(opener(match &view {
        FolderView::Select(path) => Target::Select(path),
        FolderView::Folder(path) => Target::Open(path),
    }))
}

/// What the system's program is asked to do.
#[derive(Debug, Clone, Copy)]
enum Target<'a> {
    /// Open a file with its program, or a folder in the file manager.
    Open(&'a Path),
    /// Show the file selected in its folder.
    Select(&'a Path),
}

/// The program and its arguments, one argument each.
#[derive(Debug, PartialEq, Eq)]
struct Opener {
    program: &'static str,
    args: Vec<OsString>,
    /// On Windows, `/select,` and the path must reach Explorer as one argument that Rust's
    /// quoting would split; a Windows path can't hold a `"`, so quoting it here is safe.
    raw: Option<OsString>,
}

#[cfg(windows)]
fn opener(target: Target<'_>) -> Opener {
    match target {
        Target::Open(path) => Opener {
            program: "explorer.exe",
            args: vec![path.as_os_str().to_owned()],
            raw: None,
        },
        Target::Select(path) => {
            let mut raw = OsString::from("/select,\"");
            raw.push(path.as_os_str());
            raw.push("\"");
            Opener {
                program: "explorer.exe",
                args: Vec::new(),
                raw: Some(raw),
            }
        }
    }
}

/// `xdg-open` has no way to select a file, so a file is shown by opening its folder.
#[cfg(not(windows))]
fn opener(target: Target<'_>) -> Opener {
    let path = match target {
        Target::Open(path) => path,
        Target::Select(path) => path.parent().unwrap_or(path),
    };
    Opener {
        program: "xdg-open",
        args: vec![path.as_os_str().to_owned()],
        raw: None,
    }
}

/// Starts the program and lets it run; a thread waits for it so it leaves no zombie. Explorer's
/// exit code says nothing about success, so only a failure to start is reported.
fn run(opener: Opener) -> Result<(), ErrorCode> {
    let mut command = Command::new(opener.program);
    command.args(&opener.args);
    #[cfg(windows)]
    if let Some(raw) = &opener.raw {
        use std::os::windows::process::CommandExt;
        command.raw_arg(raw);
    }
    let mut child = command.spawn().map_err(|_| ErrorCode::Io)?;
    std::thread::spawn(move || child.wait());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn explorer_opens_the_file_and_selects_it_as_one_argument() {
        let path = Path::new(r"C:\Users\me\Downloads\my file.zip");
        assert_eq!(
            opener(Target::Open(path)),
            Opener {
                program: "explorer.exe",
                args: vec![path.as_os_str().to_owned()],
                raw: None,
            }
        );
        assert_eq!(
            opener(Target::Select(path)),
            Opener {
                program: "explorer.exe",
                args: Vec::new(),
                raw: Some(r#"/select,"C:\Users\me\Downloads\my file.zip""#.into()),
            }
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn xdg_open_opens_the_file_or_the_folder_that_holds_it() {
        let path = Path::new("/home/me/Downloads/my file.zip");
        assert_eq!(
            opener(Target::Open(path)),
            Opener {
                program: "xdg-open",
                args: vec![path.as_os_str().to_owned()],
                raw: None,
            }
        );
        assert_eq!(
            opener(Target::Select(path)).args,
            vec![OsString::from("/home/me/Downloads")]
        );
    }
}
