# Matty design direction

How Matty looks and feels, and the tokens behind it. The theme itself is
`crates/matty-app/assets/theme.css` (shadcn format, so it opens in tweakcn)
plus the semantic colors in `crates/matty-app/src/style.rs`. Everything
below is applied through gpuicn's `UiTheme`; Matty never restyles a gpuicn
component, it only sets tokens and composes.

## 1. Feel

Matty should feel like a quiet native app, not a web page in a frame:

- **Calm surfaces, one accent.** Near-white or near-black canvases, a
  slightly tinted sidebar, hairline borders, and a single indigo-blue accent
  for selection, links, mentions and primary actions. Nothing else is
  saturated except presence dots and errors.
- **Text carries the hierarchy.** Weight and color, not boxes and shadows,
  separate unread from read, author from body, meta from content. Geist at
  two weights (Regular, Medium) is enough.
- **Dense but breathable.** 4px grid, 28px sidebar rows, 48px headers, 20px
  content gutters. Slack-like density, Zed-like restraint.
- **Native chrome.** Matty draws its own title bar (traffic lights inset on
  macOS, controls on Windows and client-decorated Linux) and puts navigation
  and the quick switcher in it, like Slack and Linear. Windows follow the OS
  theme by default.
- **Motion is confirmation, not decoration.** 120 to 180ms eases on hover,
  disclosure and pane collapse; nothing moves on its own. Reduced motion
  turns it all off.

## 2. Color

Tokens are shadcn names so gpuicn components pick them up unchanged.
Values are sRGB hex; the CSS is the source of truth.

| Token | Light | Dark | Used for |
| --- | --- | --- | --- |
| `background` / `foreground` | `#ffffff` / `#1b1f2a` | `#15171c` / `#e8eaef` | Channel view, dialogs' text |
| `sidebar` / `sidebar-foreground` | `#f4f5f7` / `#1b1f2a` | `#0f1114` / `#d6dae3` | Title bar, team rail, sidebar |
| `sidebar-accent` | `#e6e8ed` | `#1d2129` | Hovered and selected sidebar rows |
| `sidebar-border` / `border` | `#e3e5ea` / `#e6e8ec` | `#262a33` / `#2a2e37` | Hairlines |
| `primary` / `primary-foreground` | `#3056d3` / `#ffffff` | `#4a6de8` / `#ffffff` | Buttons, links, selected rail item, mention badges |
| `accent` / `accent-foreground` | `#eef0f4` / `#1b1f2a` | `#242830` / `#e8eaef` | Hover on buttons and menu items |
| `muted` / `muted-foreground` | `#f1f2f4` / `#5f6775` | `#1d2027` / `#8b93a5` | Timestamps, placeholders, meta |
| `card` / `popover` | `#ffffff` | `#1c1f26` | Popovers, menus, dialogs |
| `destructive` | `#dc2626` | `#f0564f` | Errors, delete, DND |
| `ring` | `#3056d3` | `#4a6de8` | Focus |
| `radius` | 8px | 8px | Base; buttons use 6px, inputs 8px, cards 10px |

Semantic colors Matty adds on top (`style::Palette`):

| Name | Light | Dark | Used for |
| --- | --- | --- | --- |
| `online` | `#16a34a` | `#3fb950` | Presence dot |
| `away` | `#f59e0b` | `#f2b038` | Presence dot |
| `dnd` | `#dc2626` | `#f0564f` | Presence dot |

Mentions, the "New messages" line and links need no token of their own:
gpuicn's `Message` tints a mention row with `primary` at 6% behind a 2px
`primary` left border, draws the "New messages" rule in `destructive`,
and Markdown links use `primary`. Matty keeps those so every gpuicn app
reads the same way.

Contrast: every foreground on its surface is at least 4.5:1, every
`muted-foreground` at least 4.5:1 on `background` and `sidebar`, and the
accent on white is 6.2:1. `style.rs` tests the pairs that matter.

## 3. Type

Geist Sans for UI and messages, Geist Mono for code. Sizes are in px at
text scale 1; everything multiplies by the user's text scale and zoom.

| Role | Size / line | Weight | Where |
| --- | --- | --- | --- |
| `title` | 15 / 20 | Medium | Channel name in the header, dialog titles |
| `body` | 14 / 21 | Regular | Post text, composer |
| `ui` | 13 / 18 | Regular | Sidebar rows, buttons, menus, title bar |
| `ui_strong` | 13 / 18 | Medium | Unread rows, author names, labels |
| `meta` | 12 / 16 | Regular | Timestamps, counts, descriptions |
| `label` | 11 / 16 | Medium, +0.4px tracking, uppercase | Sidebar category names |
| `code` | 13 / 20 | Mono | Inline code and blocks |

## 4. Layout

```
┌──────────────────────────────────────────────────────────────────────┐
│ ● ● ●  ‹ ›      [ 🔍 Search Matty                      ⌘K ]   @ 🔖 ? ◯ │ 34  title bar
├────┬───────────────┬───────────────────────────────────┬─────────────┤
│    │ Team name   ▾ │ # design  ☆   12 · 3 pinned    ⓘ  │ Thread    ✕ │ 48  headers
│ T1 │               ├───────────────────────────────────┤             │
│ T2 │ CATEGORY      │                                   │             │
│  + │  # channel  3 │  posts                            │             │
│    │  ◯ person     │                                   │             │
│    │               ├───────────────────────────────────┤             │
│    │               │ [ composer                   ➤ ] │             │
└────┴───────────────┴───────────────────────────────────┴─────────────┘
  56      240–400              ≥ 260 (flex)                  320–560
```

- **Title bar** (34px, `sidebar` background) spans the whole window. Left:
  window controls, then back and forward. Center: the quick switcher as a
  search pill, max 560px wide, that opens Cmd/Ctrl+K. Right: recent
  mentions, saved, help, and the user's avatar with a presence dot.
- **Team rail** (56px) appears only with more than one team. Teams are 32px
  rounded squares with initials or icons; the current one is filled with
  `primary` and marked with a 2px `primary` bar on the rail's edge; unread
  teams get a dot, mentions a count. A dashed square at the end joins a team.
- **Sidebar** (default 260px, 200 to 400) is the `sidebar` surface with no
  right border on the content side beyond the hairline. Header is 48px with
  the team name as a menu button. Threads and Drafts sit above the
  categories as plain rows with a `secondary` count badge when there is
  something to read or finish. Categories are collapsible 11px labels;
  rows are 28px, 8px padding, 6px radius, icon 16px. Unread rows use
  `ui_strong` in `sidebar-foreground`; read rows use `muted-foreground`;
  muted channels are 60% opacity; the selected row is `sidebar-accent`.
  Mention counts are `primary` pills.
- **Channel view** has a 48px header (name in `title`, star, member count,
  pinned count, info) over the post list and the composer. Posts use 20px
  horizontal gutters, 32px avatars, Medium author and `meta` time, grouped
  within five minutes of the same author, day dividers with a centered pill,
  and the "New messages" rule. The composer is a bordered card
  with 10px radius, 12px padding and a formatting row below the text.
- **Right pane** (default 400px, 320 to 560) shares the header height and
  separates from the channel with a hairline.
- **Empty and loading states** center a 40px icon in a tinted circle, a
  `title` line and one `meta` sentence; never more, plus at most one
  outline button. A channel still loading shows four skeleton rows shaped
  like grouped posts, bottom-aligned where the posts will land, never a
  spinner. A channel with nothing posted shows its purpose and Add members,
  sitting low over the composer. The right pane with nothing open says how
  to get a thread there.
- **Connection banner** is a 32px strip between the title bar and the
  workspace, never a dialog: `muted` with a spinner while reconnecting,
  `destructive` at 10% with a Retry button once offline. The app stays
  usable under it.

- **Sign in** (PLAN §4.5) is one 400px card on the `sidebar` surface under
  the wordmark, never a wizard. Step one asks only for the server address
  and assumes `https`. Step two shows the server as a muted chip with
  Change, then the password form, an OR rule, outline buttons for each SSO
  provider the server enables, and a personal access token behind a text
  link. Errors are one `meta` sentence in `destructive` with an icon, placed
  above the button they concern; a request in flight puts a spinner in the
  primary button and never blocks the window.
- **Post actions** live in the hover toolbar: add reaction, reply in
  thread, save (the bookmark fills when saved) and a More menu. Add
  reaction opens a menu of the six quick reactions with the user's own
  checked, until the emoji data arrives for the full picker. More holds
  Copy text, Pin/Unpin, Save/Unsave and, on the user's own posts, Edit and
  Delete. Reactions, saves and pins apply in the view at once and a chip
  that drops to zero disappears. Delete asks in an alert dialog whose
  button says Delete (PLAN §4.5); nothing destructive happens from a menu
  alone.
- **Composer feedback** (PLAN §4.3): Enter or the send button appends the
  post at once as a row by the current user at 60% opacity with "Sending…"
  for its time; the composer clears without waiting for the server. Send is
  disabled while the composer is empty or over the limit. A row the server
  refused keeps its text, says "Message failed to send." in `destructive`
  with an icon, and offers Retry and Delete as text links; nothing is lost
  and nothing pops up. A `meta` counter ("189 / 200") appears next to Send
  only in the last tenth of the server's `MaxPostSize` and turns
  `destructive` over it. The B, I and code buttons wrap the selection in
  Markdown and leave focus in the composer; Up in an empty composer edits
  the user's last post, Shift+Up replies to the newest one.
- **Thread pane replies** behave like the channel composer: Enter or the
  send button adds the reply as a "Sending…" row at 60% under the thread,
  the replies count follows, a failed reply offers Retry and Delete in
  place, and Send is disabled while empty. Follow/Following is a small
  outline or secondary button on the replies rule, never a toggle switch;
  it flips at once.
- **Reply footer** (Collapsed Reply Threads, PLAN §4.4): a post with a
  thread shows one line under its body: up to three small overlapping
  avatars of who replied, "3 replies" in `primary` and `ui_strong`, a
  `primary` dot when there are unread replies, "Last reply 08:13" in
  `meta`, and a ghost Follow/Following button. The whole line is a hover
  target that opens the thread; replies never appear inline in the
  channel.
- **Preferences** is a dialog, not a window: sections under `label`
  headings, a joined toggle group for theme (System, Light, Dark) and text
  size (90 to 130 percent), switches with a one-line `meta` explanation.
  Every change applies behind the dialog as it is made; the only button is
  Done.

## 5. Components

- Every control is a gpuicn component with default styling; Matty passes
  tokens and `Styled` geometry only. If a look is impossible without
  restyling, it becomes a gpuicn issue, not a local override.
- Icons are Lucide at 16px in chrome, 20px in empty states, 14px inside
  badges and sidebar rows. Stroke color follows the text color around it.
- Tooltips on every icon-only button, with the shortcut when there is one.

## 6. Screens that set the bar

Screenshots live in `docs/screenshots/`; the `design-*` files are the
reference for each pull request in this series. A change that moves away
from them needs a reason in its pull request.

To look at the design before sync exists, run the app with
`MATTY_PREVIEW=1`: the views render the fixtures in
`crates/matty-app/src/preview.rs` instead of their loading and empty
states. The `design-*` screenshots are taken this way.
