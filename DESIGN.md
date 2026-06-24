# Design System

ShovelsUp design language for iOS, Android, and web.

## Colors

Two brand colors, sourced directly from `logo.svg`:

| Token | Hex | Role |
|-------|-----|------|
| Brand Primary | `#E84E0F` | Primary actions, links, highlights |
| Foreground Primary | `#1B1F22` | Body text, headings |

### iOS

Defined as asset catalog color sets; reference via the `Color+Brand.swift` extension:

```swift
.foregroundStyle(.brandPrimary)       // #E84E0F
.foregroundStyle(.foregroundPrimary)  // #1B1F22
```

Never use hardcoded color literals (`Color.orange`, `Color(hex:)`, etc.).

### Android

Defined in `app/src/main/res/values/colors.xml`:

```xml
@color/brand_primary       <!-- #E84E0F -->
@color/foreground_primary  <!-- #1B1F22 -->
```

Reference via the Material3 theme — add to `Color.kt` and wire into `ShovelsUpTheme` before using directly in composables.

### Web

Defined as CSS custom properties in `static/css/main.css`:

```css
var(--color-primary)  /* #E84E0F */
var(--color-text)     /* #1B1F22 */
```

Never hardcode hex values in templates or component styles.

## Typography

Each platform uses its native system font stack. No custom typefaces.

| Platform | Font |
|----------|------|
| iOS | SF Pro (SwiftUI default) |
| Android | Roboto (Material3 default) |
| Web | `system-ui, -apple-system, sans-serif` |

The logo wordmark uses Helvetica Neue — this is **display only**, not used in UI.

## Logo

`logo.svg` in the repo root is the source of truth. Use it as-is; do not recolor or modify.

## Icons

`icon.svg` in the repo root is the square shovel-mark icon (white background, `#E84E0F` shovel, 200×200 viewBox). It is the source for all app icons and favicons. Regenerate platform assets from it with `rsvg-convert` when it changes.

### iOS

`AppIcon.png` (1024×1024) lives in `Assets.xcassets/AppIcon.appiconset/`. Regenerate:

```bash
rsvg-convert -w 1024 -h 1024 icon.svg -o apps/ios/ShovelsUp/Assets.xcassets/AppIcon.appiconset/AppIcon.png
```

### Android

`ic_launcher.png` lives in each `mipmap-*` directory. Regenerate all densities:

```bash
rsvg-convert -w 48  -h 48  icon.svg -o apps/android/app/src/main/res/mipmap-mdpi/ic_launcher.png
rsvg-convert -w 72  -h 72  icon.svg -o apps/android/app/src/main/res/mipmap-hdpi/ic_launcher.png
rsvg-convert -w 96  -h 96  icon.svg -o apps/android/app/src/main/res/mipmap-xhdpi/ic_launcher.png
rsvg-convert -w 144 -h 144 icon.svg -o apps/android/app/src/main/res/mipmap-xxhdpi/ic_launcher.png
rsvg-convert -w 192 -h 192 icon.svg -o apps/android/app/src/main/res/mipmap-xxxhdpi/ic_launcher.png
```

### Web

The SVG logo is used directly as the favicon — no rasterization needed:

```html
<link rel="icon" href="/static/logo.svg" type="image/svg+xml">
```
