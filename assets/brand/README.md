# Velocity brand assets

The source artwork for Velocity's icon and wordmark. It is V1's artwork, which the Product Owner authorised in V1 issue #35 and approved for V2 on #29 (2026-10-29). That isn't a release approval.

| File | Use |
|---|---|
| `velocity-icon.svg` | The app icon, at 48 px and larger |
| `velocity-icon-small.svg` | The app icon, simplified for 16 to 32 px: no sheen, shadow or speed cuts |
| `velocity-mark.svg` | The glyph alone, on a transparent background |
| `velocity-lockup-light.svg`, `velocity-lockup-dark.svg` | The icon and wordmark, for light and dark backgrounds |

The app draws the small icon inline (`src/components/BrandIcon.tsx`) in the sidebar and on the first-run screen.

## Regenerating the icons

The desktop icons in `src-tauri/icons/` are generated from these files, never edited by hand. Write the output to a temporary folder, then copy only the files `src-tauri/icons/` already has:

```sh
pnpm tauri icon assets/brand/velocity-icon.svg -o <tmp>/full
pnpm tauri icon assets/brand/velocity-icon-small.svg -o <tmp>/small -p 32
```

- **`32x32.png`** comes from the small icon, and every other file from the full icon.
- **`icon.ico`** is Tauri's, so all its sizes come from the full icon. V1 built its `.ico` with the 16, 24 and 32 px sizes from the small icon, which needs ImageMagick. That is left for packaging (#81).

## Before a public release

The lockups set the wordmark as live text in Inter, so they look different where Inter isn't installed. Convert the text to outlines before using a lockup outside the app.
