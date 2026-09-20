# lqxp — agent instructions

## Visual verification workflow (mandatory for frontend/UI tasks)

The rendered result is the source of truth. Never consider a frontend task
complete merely because the HTML/CSS/TypeScript looks correct.

### Browser integration

- OpenCode drives the user's existing Chromium via the
  `@vymalo/opencode-browser` plugin + "OpenCode Browser" extension
  (ID `cabnfapnafjlijmbpmgjkgobhdkbmpci`) over a localhost WebSocket bridge.
- Bridge: `ws://127.0.0.1:4517`. Token lives in
  `~/.local/state/opencode-browser/bridge.json` (0600).
  Paste the URL + token into the extension dashboard → Save & reconnect.
- Global OpenCode config (`~/.config/opencode/opencode.jsonc`) registers the
  plugin with `groups: ["page", "control", "debug"]` so `browser_console` and
  `browser_set_viewport` are available.
- The bridge only lives while an OpenCode process runs (TUI / `opencode web` /
  `opencode run`). If `browser_targets` is empty or `browser_open` fails with
  "no browser extension is connected", the extension is not connected —
  re-check its dashboard, do not invent another browser stack.
- Do NOT install a different browser MCP (Playwright MCP, CDP hacks, etc.)
  as a substitute. Headless Chromium screenshots are a fallback for
  environment checks only, not the primary loop.

### Frontend loop (every UI change)

1. Modify the code in `web/`.
2. Serve the build under test: the source of truth is
   `http://127.0.0.1:4560/app/` (Rust backend serving `web/dist/`,
   rebuilt by the watcher). Port 4173 serves `lqxp-client` (a different
   project) — do NOT verify against it. Confirm with
   `curl -s -o /dev/null -w "%{http_code}\n" http://127.0.0.1:4560/app/`.
3. Open/refresh the affected page in the browser:
   `browser_open({ group: "lqxp", url: "http://127.0.0.1:4560/app/" })` or
   `browser_navigate` + `browser_reload`.
4. `browser_snapshot` first, prefer stable refs (`e1`, `e2`…) over selectors.
5. `browser_screenshot` (writes to `.opencode/browser/<group>/…png`), then
   `read` the PNG to actually SEE it.
6. `browser_console` — fix errors/warnings (known benign: Vite
   connecting/connected; known warning to watch: Vue `inject()` outside
   `setup()`).
7. Interact: `browser_click`, `browser_type`/`browser_fill`,
   `browser_scroll`, `browser_press_key`, `browser_wait` for selectors.
8. Responsive check when layout is touched (CDP/Chromium only):
   `browser_set_viewport` at 1440px (desktop), 768px (tablet), 390px (mobile).
9. Fix regressions, repeat until rendered result is correct.

### What to look for

- Elements overlapping, broken flex/grid, wrong widths/heights, overflow,
  clipped content, bad spacing, broken typography, missing elements,
  z-index problems, horizontal scrolling, elements outside their container.
- Splash overlay (`#splash`) must hide after mount — headless screenshots
  taken before JS settles can show a ghost logo; use
  `browser_wait` / virtual-time-budget before judging.
- No horizontal overflow: `document.documentElement.scrollWidth` must equal
  `clientWidth` at 1440 / 768 / 390.

### Verification checklist (end of each frontend task)

- [ ] Correct origin verified (`4560/app/`, not `4173`)
- [ ] `dist/` fresh (timestamp / marker string present in built assets)
- [ ] Page opened/refreshed in the real browser via the bridge
- [ ] Screenshot taken and visually inspected via `read`
      (if `browser_screenshot` times out on the logged-in app, fall back to
      rect-based verification below and note it)
- [ ] Console checked (`browser_console`), no new errors
- [ ] Responsive viewports checked when relevant (1440 / 768 / 390)
      — viewport emulation verified by measuring `.app` rect, not by ACK
- [ ] Interaction verified (click/type/scroll) when behavior changed
- [ ] No leftover test state (test rooms/channels removed, nav state sane)
- [ ] No other scoped/global CSS trap introduced (see notes)
- [ ] All user-facing strings go through `t()` — zero hardcoded text in
      templates or script (see i18n section below)

### Project-specific verification notes (learned the hard way)

#### Ports and builds

- `4560` = Rust backend serving `web/dist/`. `4173` = `lqxp-client`
  dev server (different project). `4174`+ = ad-hoc — never verify on those.
- `web/dist/` is rebuilt by `vite build --watch` (+ `inject --watch`).
  The watcher can hang after rapid successive rebuilds: symptom is
  `dist/` emptied or missing while the process is still listed. Recovery:
  kill leftover watchers (never run two concurrently — they race on
  `outDir`), run one-shot `bun run build` in `web/`, then restart a fresh
  detached watcher with a log file. A `bun run build` exiting 0 plus
  `curl 200` on `/app/` is the minimum bar before any browser check.
- After each edit, confirm the running build picked it up: check the
  `dist/assets/*` timestamp/hash, or grep the built CSS/JS for a marker
  string from the change. Hashed filenames only change when content
  changes — same hash after an edit means the edit did not rebuild.

#### Viewport emulation (mobile)

- `browser_set_viewport` through the extension bridge only takes effect
  together with `mobile: true` **and a `browser_reload` afterwards**
  (matchMedia re-evaluates on load). Never trust the ACK: verify by
  measuring `.app` rect via `browser_get_attribute` (expect width 390).
- Reset to `1912×857` when done so the tab is left in a sane state.

#### Rect-based verification (when screenshots fail)

- `browser_screenshot` can time out persistently on the logged-in app
  (heavy page); `browser_eval` / `browser_console` can fail with bridge
  errors (`debugger already attached`, internal `c.split`). Do not block
  on them: verify numerically instead.
- `browser_query` skips `display: none` elements; `browser_get_attribute`
  returns `rect` + `class` + `data-v-*` attrs. Layout math must add up,
  e.g. titlebar + side + foot heights equal the viewport height, and
  x/width of rail + panel + thread equal the viewport width (mind the
  `windowZoom` factor visible in fractional rects).
- All-zero rects on a subtree whose parent measures fine means the
  subtree is collapsed — bisect by measuring intermediate ancestors
  (`.thread__shell`, `.thread__main`, …). Beware transients: a partially
  written `dist/` (watcher race) serves truncated CSS and produces bogus
  zeros — rebuild cleanly and re-measure before blaming the CSS.

#### Vue scoped-CSS traps (must-check for every layout change)

- Scoped selectors in `InboxView.vue` NEVER match elements rendered by
  child components (`MessengerSidebar`, `ServerChannelPanel`, …) because
  of differing `data-v-*` attributes. Grid placement (`grid-row` /
  `grid-column`), mobile `display` toggles, and context-menu styles for
  `.side`, `.side__foot`, `.chanpanel`, `.room-context*` MUST live in
  global `styles.css` (or in the rendering component's own scoped style).
- Same trap for specificity: a component-scoped `display: flex` (e.g.
  `.chanpanel`) beats a global mobile `display: none`. Reinforce global
  toggles, e.g. `.app:not(.is-channels) > .chanpanel { display: none; }`,
  and verify the hidden state by the element's absence from
  `browser_query` results.
- After touching these areas, inspect the `data-v-*` attributes in
  `browser_get_html` output to confirm which scope each element has.

#### Bridge interaction limits

- Synthetic right-click (`browser_click` with `button: right`) does NOT
  open Vue `contextmenu` menus. Verify context menus by code inspection
  or ask the user to long-press on device; prefer destructive actions
  with plain-button paths (e.g. thread "Leave room" + confirm dialog).
- Touch/swipe cannot be performed through the bridge. Swipe logic must be
  verified by code review + tapping the equivalent buttons (back buttons,
  channel rows); real swipe testing stays manual on device.
- The driven tab shares the user's session and the user may be active in
  parallel (state can change between two tool calls). Prefer reload-driven
  state, keep clicks minimal, restore navigation afterwards, and clean up
  test rooms/channels/accounts created during verification.

### i18n — mandatory, no exceptions

**Every user-facing string must go through the i18n system.** Hardcoded
text in templates or script is a bug, period.

#### System overview

- Custom composable (NOT vue-i18n): `web/src/composables/useI18n.ts`
- Translation files: `web/src/i18n/{en,fr,es,ru}.json`
- Locales: `en` (source), `fr`, `es`, `ru`
- Interpolation syntax: `{variableName}` (curly braces, NOT vue-i18n)

#### How to use in components

```typescript
const { t } = inject<ReturnType<typeof useI18n>>("i18n") ?? useI18n();
```

```html
<template>
  <p>{{ t('ban.roomMessage', { channel: channelLabel }) }}</p>
</template>
```

#### Rules

1. **Never hardcode** a user-facing string (button labels, error messages,
   placeholders, aria-labels, tooltips, status text, toast messages, etc.).
   If it appears on screen or is read aloud, it must be a `t()` call.
2. **Add keys to `en.json` first**, then copy the same key structure to
   `fr.json`, `es.json`, `ru.json`. All four files must stay in sync.
3. **Use descriptive, namespaced keys** — follow the existing pattern:
   `section.subsection.key`, e.g. `composer.muteRemaining`,
   `capWidget.verify`, `settings.about.version`.
4. **Never use brand names as hardcoded text** — if "QxChat" must appear,
   add it as a translation key (e.g. `app.brand`).
5. **Fallback strings in code are also forbidden** — patterns like
   `t("key") || "fallback text"` must not contain user-visible text.
   Use empty string or a dedicated i18n key for the fallback.
6. **Accessibility strings count** — `aria-label`, `title`, `alt` attributes
   must use `t()` or `:aria-label="t('...')"` binding.
7. **Format strings (dates, durations)** must use locale-aware formatters
   (`Intl.DateTimeFormat`, `Intl.RelativeTimeFormat`, etc.) or dedicated
   i18n keys — not English-only templates like `"2h 15m"`.
8. **When editing a file with hardcoded strings**, fix them all — don't
   leave a mix of `t()` and raw text in the same component.

#### Checklist for every PR touching UI

- [ ] Grep for hardcoded strings in changed `.vue` and `.ts` files:
      `grep -nE '"[A-Z][a-z].*"' --include="*.vue" --include="*.ts" src/`
- [ ] All new keys added to all 4 locale files (`en`, `fr`, `es`, `ru`)
- [ ] No `|| "fallback"` with English/French text after a `t()` call
- [ ] Aria-labels, title attrs, and alt text use `t()`
