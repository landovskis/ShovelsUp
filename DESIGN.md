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

## Cards

Cards present one project, permit, decision, or compact group of related facts. They should feel like civic records: structured, direct, and slightly editorial rather than soft or decorative.

### Principles

- Give each card one subject and one primary action.
- Use alignment, rules, labels, and spacing to create hierarchy before adding elevation.
- Keep corners restrained. Cards are records, not floating bubbles.
- Use Brand Primary for an active state, status marker, or accent rule — never as a large card background.
- Do not nest cards. Use dividers or grouped rows inside a card instead.
- Keep metadata labels short, uppercase where space permits, and visually secondary to their values.

### Anatomy

A card may contain these regions, in order:

1. **Topline** — category on the left and status on the right.
2. **Identifier** — permit or file number in compact secondary text.
3. **Title** — address, project name, or decision.
4. **Metadata** — two to four labelled facts separated from the title by a rule.
5. **Progress or action** — optional timeline, disclosure, or single primary action.

Omit empty regions rather than leaving placeholders. The title is the only required region.

### Card variants

| Variant | Use | Treatment |
|---------|-----|-----------|
| Standard | Search results, project lists, summaries | Surface background, 1 px neutral border, 8 px corners, no shadow by default |
| Record | Featured permit or project dossier | Surface background, 6 px Brand Primary top rule, strong elevation, 8 px corners maximum |
| Decision | Compact council decision or status callout | Foreground Primary background, light text, Brand Primary circular status mark |
| Map overlay | A record positioned over a map or spatial view | Use the Record treatment; overlap the map deliberately and keep the card fully opaque |

Do not create additional variants for small visual differences. Start with Standard and promote to Record only when the item is the page's focal object.

### Spacing and shape

| Token | Compact | Regular |
|-------|---------|---------|
| Internal padding | 16 px | 24 px |
| Region gap | 12 px | 16 px |
| Metadata top margin | 16 px | 24 px |
| Corner radius | 8 px | 8 px |
| Accent rule | 4 px | 6 px |

Use Compact below 480 px or in dense lists. Use Regular for standalone and featured cards. Larger cards may use up to 40 px internal padding, but the corner radius does not increase with card size.

### Interaction

- If the whole card navigates, expose it as one semantic link and do not place competing links inside it.
- If only one action is available, keep the card static and use a clearly labelled button or link.
- Hover may lift an interactive card by no more than 2 px. Never rely on hover alone to communicate interactivity.
- Keyboard focus must be clearly visible around the complete interactive target.
- Status must always be written as text. Color, dots, and progress bars are supporting cues only.
- Motion must respect the platform's reduced-motion setting.

### Platform implementation

#### iOS

Use system background and separator colors for Standard cards. Apply `.brandPrimary` and `.foregroundPrimary` through the existing brand color extension. Preserve the spacing, restrained corner radius, and variant hierarchy above when implementing a reusable card view.

#### Android

Use Material theme surface, outline, and content colors for Standard cards. Route Brand Primary and Foreground Primary through `ShovelsUpTheme` before use. Preserve the spacing, restrained corner radius, and variant hierarchy above when implementing a reusable composable.

#### Web

Use the semantic CSS custom properties already defined in `static/css/main.css`:

```css
background: var(--color-surface);
color: var(--color-text);
border-color: var(--color-line);
```

The landing-page `.permit-card` is the reference Record card and `.decision-card` is the reference Decision card. New card styles must reuse tokens; never hardcode color literals in templates or component styles.

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
