# Categories

Categories sort downloads by file type, and can give each type its own folder. Open **Categories** in the navigation.

## The built-in categories

| Category | File types |
|---|---|
| Compressed | zip, rar, 7z, tar, gz, tgz, bz2, xz, zst, cab, arj, lzh |
| Documents | pdf, doc, docx, xls, xlsx, ppt, pptx, odt, ods, odp, txt, rtf, epub, csv |
| Music | mp3, flac, wav, aac, ogg, opus, m4a, wma, aiff |
| Video | mp4, mkv, avi, mov, wmv, webm, m4v, flv, mpg, mpeg, ts |
| Programs | exe, msi, msix, appx, dmg, pkg, deb, rpm, appimage, apk, flatpak, snap |
| Disk images | iso, img, vhd, vhdx, vmdk, qcow2 |

Built-in categories can be edited but not deleted.

## Your own categories

**New category** asks for a name (up to 40 characters), a colour, an icon, the file extensions it takes (separated by spaces or commas, such as `zip 7z rar`), and optionally a folder to save to. A category without extensions takes only downloads you give it yourself.

**Deleting** a category asks first. Its downloads are left without a category; no file is moved or deleted.

## Where a download goes

When you add a download, Velocity chooses:

1. **The category:** the one you choose. Otherwise, with **automatic categories** on, the one for the file's extension; a torrent takes the category of its largest file.
2. **The folder:** the one you choose. Otherwise the category's own folder. Otherwise the default download folder, in a subfolder named after the category when **category subfolders** are on.

Both switches are in Settings, Downloads, and both are on by default. A missing category subfolder is created when you add the download; any other missing folder is offered in the Add dialog as **Create this folder**.

You can change a download's category later in its details panel. That changes how it is listed, and doesn't move its file.
