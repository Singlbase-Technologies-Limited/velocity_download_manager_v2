# File names, categories and folders

How V2 names a download's file, chooses its category and folder, and what it does when the name is taken (#31, built on #40). The rules are V1's, with its fixes for V1 issues #38 and #46, adapted to names that the V2 engine learns from the server. The code:

- [`naming.rs`](../crates/velocity-core/src/naming.rs): file names.
- [`category.rs`](../crates/velocity-core/src/category.rs): categories.
- [`destination.rs`](../crates/velocity-core/src/destination.rs): folders, the folder settings and what is already there.
- [`manager/categories.rs`](../crates/velocity-core/src/manager/categories.rs): where they meet the database and the list.

## File names

Every name from outside, whether a link, a server or a typed name, becomes one plain path component on every supported platform (`sanitize_file_name`):

- **Characters:** separators, control characters and the characters Windows forbids (`< > : " | ? *`) become `_`.
- **Ends:** leading dots go, and so do trailing dots and spaces.
- **Reserved names:** a name Windows reserves for a device, with or without an extension (`CON`, `con.txt`, `LPT¹.zip`), gets a leading `_`.
- **Part files:** a name ending in `.vdmpart`, the engine's suffix for a file being downloaded, gets a trailing `_`.
- **Length:** a name longer than 240 bytes is shortened, keeping its extension.
- **Nothing left:** a name with nothing usable left becomes `download`.

**The name a new download gets** (`name_for_download`):

| The user… | The link | Saved as |
|---|---|---|
| typed `Annual report` | `…/report.pdf` | `Annual report.pdf`: a typed name keeps the link's file type (V1 #46) |
| typed `notes.txt` | `…/report.pdf` | `notes.txt`: a typed name that already has the link's extension, or one a category lists, is kept as typed |
| typed nothing | `…/My%20File.iso?x=1` | `My File.iso`: the link's last path segment, decoded |
| typed nothing | `…/files/12345`, `…/download.php?id=3` | Named by the server: a link without an extension, or ending in a server script (`.php`, `.asp`, `.aspx`, `.jsp`, `.cgi`), is named by `Content-Disposition`, else by the final URL |

A typed name that isn't already a safe file name is refused with `INVALID_FILENAME`, rather than changed behind the user's back.

**The Add form's preview:** `src/naming.ts` mirrors `name_for_download` for #60's preview. `naming-cases.json`, which Rust generates, holds the cases both sides must agree on, so the preview can't drift from what adding does. The native side always decides.

## Categories

**The built-in categories** came with schema 6:

| Category | Its extensions |
|---|---|
| Compressed | zip, rar, 7z, tar, gz, tgz, bz2, xz, zst, cab, arj, lzh |
| Documents | pdf, doc, docx, xls, xlsx, ppt, pptx, odt, ods, odp, txt, rtf, epub, csv |
| Music | mp3, flac, wav, aac, ogg, opus, m4a, wma, aiff |
| Video | mp4, mkv, avi, mov, wmv, webm, m4v, flv, mpg, mpeg, ts |
| Programs | exe, msi, msix, appx, dmg, pkg, deb, rpm, appimage, apk, flatpak, snap |
| Disk images | iso, img, vhd, vhdx, vmdk, qcow2 |

**Built-in categories** can be edited but not deleted (`INVALID_STATE`). **User categories** get ids `c-1`, `c-2` and so on.

**A category's rules** are V1's:

- **Name:** 1 to 40 characters, with no control characters. It must be unique, whatever its case: `Курсы` and `курсы` clash, which SQLite's `NOCASE` alone wouldn't catch (`NAME_TAKEN`).
- **Colour:** `#rrggbb`.
- **Icon:** one of twelve.
- **Extensions:** at most 100, each up to 16 letters or digits, stored lowercase without the dot.
- **Folder:** optional, and absolute.

**The automatic category** is the first category, in the order shown, that lists the file name's extension.

**Changing a download's category** (`set_download_category`) moves nothing. **Deleting a category** leaves its downloads without one, and touches no file.

## Folders

**The category and the folder** are chosen as V1 chooses them since its #38:

1. **The category:** the one chosen. Otherwise, with automatic categories on, the one for the file name's extension.
2. **The folder:** the one chosen. Otherwise the category's own folder. Otherwise the default folder, in a subfolder named after the category (a safe file name) when category subfolders are on.

**The folder settings** are the settings document's `downloads` section (`FolderSettings`). [SETTINGS.md](SETTINGS.md#folders) lists them.

**A folder path** must be absolute, at most 4096 bytes, with no control character and no `..` part, with either separator on any platform (`INVALID_PATH`). Only the text is checked; following links, and the other file system checks, are #72's.

**When adding,** a missing category subfolder is created inside its parent, one level, and never through a link (`metalink::create_folders`; a link there is `INVALID_PATH`). Any other missing folder is `DESTINATION_MISSING`: the default folder, a category's own folder, and a chosen folder.

## A name learned after the start

When the server names the file, the engine reports the name once the download has started.

- **The category:** if the download has no category and automatic categories are on, it gets the category for that name.
- **The folder:** the file stays in the folder it started in (PO decision on #31). For a link the server names, that's the default folder.

## A name that is taken

**A name is taken** by any of these:

- anything at it: a file, a folder or a link, even a broken one;
- its V2 part file (`.vdmpart`);
- V1's control file (`.aria2`), which V1 keeps beside a download it hasn't finished;
- a download in the list that hasn't finished and is to be saved under it.

**What adding does** is the user's choice (`OnConflict`, PO decision on #31):

- **`ask`:** add nothing, and report what's there with the first free name (`name (1).ext`).
- **`rename`:** use the first free name.

**An existing file is never replaced.** V1's *Overwrite* isn't carried over, and neither is its *Resume*: V1's partial downloads are aria2's, and V2 never reads them. So when V1 and V2 save into the same folder, neither writes over the other's file.

**The engine keeps the same rule** for the server's names (#40). A name the server gives that is taken gets the first free one, and the finished file is renamed into place without replacing anything ([ENGINE_INTERNALS.md](ENGINE_INTERNALS.md)).

## What's not here yet

- **Free disk space:** `check_destination` doesn't report it yet. V1 used the `fs4` crate, and a new dependency needs the PO's approval.
- **Creating a chosen folder:** a folder the user picks must already exist. Offering to create one is for #60's form, with #72's file system checks.
- **Torrent categories:** a torrent gets a category only when one is given when it is added. Choosing one from its metadata is for the Add form (#60).
- **The screens:** the Categories screen came with the sidebar (#29, [UI_ARCHITECTURE.md](UI_ARCHITECTURE.md)), and the conflict question comes with the Add form (#60) (PO decision on #31).
