# Daggy interface system

## 1. Visual theme and atmosphere

Daggy is a quiet graphite storage instrument. The chrome recedes into precise,
low-contrast mineral surfaces; the allocation map is the only saturated area.
It should feel calm during a long scan and exact when comparing sizes.

## 2. Color palette and roles

| Token | Value | Role |
|---|---:|---|
| Canvas | `#0D0F12` | Primary map background |
| Surface | `#14171C` | Header, inspector, status bar |
| Elevated | `#1A1E24` | Buttons and raised controls |
| Divider | `#292E36` | One-pixel structural separation |
| Text | `#E7E9EC` | Primary labels and values |
| Secondary | `#9299A3` | Supporting labels |
| Muted | `#626A75` | Shortcuts and tertiary metadata |
| Warm focus | `#E7A65A` | Primary branch and folder identity |

Branch colors use copper, moss, slate, clay, and ochre. There are no gradients,
glass materials, or generic semantic rainbow colors.

## 3. Typography rules

- Interface: SF Pro Text, 12 to 14 px for dense controls and rows.
- Values: SF Mono, tabular by construction, 12 to 24 px.
- Focus title: 20 px semibold.
- Empty-state title: 26 px semibold.
- Sentence case everywhere.

## 4. Component styling

- Buttons: 40 px minimum height, 8 px radius, solid surface step, brief opacity
  change while pressed.
- Rows: 44 px, one-pixel separators, no cards. Selection changes the whole row
  surface; hover uses a quieter step.
- Allocation map: 28% center, three visible rings, 1.2 px radial gaps, segments
  thinner than 6 px at the ring midline grouped.
- Status: text plus a single colored dot, never badge or pill collections.

## 5. Layout principles

The window is a two-part instrument: a flexible map and a fixed 408 px ranked
inspector. Header and status rows orient without competing with the data.
Spacing follows 4, 8, 10, 14, 16, 20, 24, and 28 px increments.

## 6. Depth and elevation

Depth uses background lightness steps only. Shadows are absent because they are
ineffective on the dark canvas. Dividers are one pixel and low contrast.

## 7. Do and do not

- Do label cached, partial, complete, and unreadable states explicitly.
- Do cross-highlight the map and ranked rows.
- Do keep exact sizes right-aligned in SF Mono.
- Do group visually insignificant wedges.
- Do not use emoji, glass, gradients, glow, or decorative animation.
- Do not make cached totals look current.
- Do not animate keyboard navigation.

## 8. Responsive behavior

The native window has a 940 by 620 minimum. The map absorbs width changes while
the inspector remains fixed, preserving numeric alignment and useful hit areas.

## 9. Agent prompt guide

- Add a control on `#14171C`, 40 px high, 8 px radius, SF Pro Text 13 px medium,
  `#E7E9EC` label, and `#20262E` hover.
- Add a metric using SF Mono 24 px medium in `#E7E9EC`, with a 12 px supporting
  label in `#626A75`.
- Add an inspector row at 44 px, 16 px horizontal padding, `#20242A` bottom
  divider, and right-aligned SF Mono values.
- Add a map branch using the mineral palette, 1.2 px gaps, no shadow, and a
  brighter solid fill only for the active segment.
