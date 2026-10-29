# Matty — plan of action

Matty is a native Mattermost desktop client written in Rust on GPUI, built from
[gpuicn](https://github.com/devaryakjha/gpuicn) components and
[gpui-icons](https://github.com/devaryakjha/gpui-icons). This document is the
plan for getting it from an empty repository to a client that anyone on a
Mattermost server can install and use all day instead of the official Electron
app or the browser.

Written 28 October 2026 against gpuicn v0.5.0, GPUI Kit 0.8.1, `gpui-pre`
0.3.10, gpui-icons 0.5.0 (Lucide 1.63.0) and Rust 1.98. Recheck the pins
before starting each milestone; do not upgrade them silently.

## 1. What "usable for everyone" means

A person who uses Mattermost every day can switch to Matty and not want to go
back. Concretely:

1. **It does the daily job.** Sign in (password, LDAP, MFA, SSO), find and
   join channels, start DMs, read and send in channels, DMs and group
   messages, follow threads, react, edit, delete, pin, save, mention, search,
   upload and preview files, and get notified. Missing any of these sends
   people back to the official app on day one, so all of them are in M1.
2. **It feels like a desktop chat app.** Layout, shortcuts and behavior follow
   what Slack, the Mattermost desktop app, Discord and Telegram Desktop have
   already taught people. Nobody should have to learn Matty.
3. **It is noticeably better at something.** Native means fast cold start,
   low memory, instant channel switching, smooth scrolling through long
   history, and keyboard-first navigation. That is the reason to switch, so it
   is measured, not claimed (section 9).
4. **It is trustworthy.** Tokens in the OS keychain, nothing lost when the
   network drops, drafts survive restarts, and it never marks something read
   that you did not see.
5. **It is accessible and installable.** Signed installers for macOS, Windows
   and Linux, auto-update, screen reader and keyboard support, and respect for
   system theme, text size and reduced motion.

### Product principles

- **Conventions over invention.** When Slack, Mattermost and Discord agree on
  a behavior (Enter sends, Up edits your last message, Cmd/Ctrl+K switches
  channels), copy it. Spend novelty only on visual polish and speed.
- **Never block on the network.** Render from the local cache first, then
  reconcile. Sending is optimistic with visible pending and failed states.
- **Server truth wins.** The cache is a cache. Unread state, permissions and
  config come from the server; Matty never invents state the web app would
  disagree with.
- **Degrade honestly.** Features Matty does not support yet (webapp plugins,
  Boards, Calls) get a clear "Open in browser" path instead of silently missing
  content.
- **Reusable UI comes from gpuicn.** Arya is building the missing reusable
  components (section 2) in gpuicn's styling. Matty consumes them and never
  carries its own one-off substitutes. Mattermost models, API calls and app
  state stay in Matty.

## 2. What we build on

### Available today

**gpuicn v0.5.0 (45 components).** Theme (tokens for color, type, radius,
density and motion, plus shadcn CSS import), Button, Input, Textarea,
InputGroup, Field/Form, Select, Combobox, Autocomplete, Menu, ContextMenu,
Menubar, Dialog, AlertDialog, Drawer, Popover, Tooltip, PreviewCard, Toast,
Tabs, Toggle/ToggleGroup, Switch, Checkbox, RadioGroup, Avatar, Progress,
Meter, Empty, ScrollArea, Separator, Collapsible, Accordion, Toolbar,
NavigationMenu, Slider, NumberField and others (the registry is the full
list), **Sidebar** (groups, badges, icon rail, collapse, actions), **Resizable**
(nested, controlled, keyboard resize) and **VirtualList** (fixed-height rows,
stable IDs, single and multi selection). The showcase app already composes
SidebarLayout, Resizable, Input and a local message log, which is a useful
starting skeleton.

**GPUI Kit 0.8.1 primitives that gpuicn does not style yet.** These are the
big ones for a chat client, and they exist upstream:

| Kit primitive | Use in Matty |
| --- | --- |
| `MessageScroller` | Variable-height virtualized message list with tail-follow, `prepend` for history paging, `scroll_to_item`, jump-to-latest button and an accessible `Log` role |
| `Message`, `MessageGroup` | Message row slots (avatar, header, body, footer) and grouping by sender. Kit's reaction region (`BubbleReactions`) belongs to `Bubble`, which Matty does not use for left-aligned team chat, so G1 needs its own reaction-chip row |
| `Attachment` | File and image cards with upload progress, retry and remove |
| `TextView::markdown` with plugins | Rendering Mattermost Markdown, with plugins for @mentions, ~channels, emoji and custom links |
| `Command` | Quick switcher (Cmd/Ctrl+K) and command palette |
| `NativeMenu`, `TitleBar` | OS app menu, custom title bar with traffic lights |
| `Kbd`, `Badge`, `Tag`, `Spinner`, `Skeleton`, `HoverCard` | Shortcut hints, unread and mention counts, status chips, loading states, profile popovers |
| `highlighter` (`syntect` feature) | Syntax highlighting in code blocks |

**gpui-icons 0.5.0.** All 1,924 Lucide icons embedded, no runtime fetching.
Enough for every chrome icon Matty needs.

### Components Matty needs from gpuicn

Arya is adding these to gpuicn, in gpuicn's styling, with the evidence gpuicn
already requires. They are not Matty work items: this table is Matty's list of
requirements and the milestone that needs each one, so the two projects can be
sequenced. Matty's job is to state what it needs precisely (the behaviors in section 4
and the findings of spikes S1 and S4) and to report gaps it finds while
integrating. Until a component lands, Matty screens that need it are built
against the gpuicn branch or wait; they do not get a temporary local copy.
During an alpha Matty may pin a merged gpuicn commit; every beta or stable
Matty release pins a tagged gpuicn release.

| Component | Likely Kit base | Needed by |
| --- | --- | --- |
| G1. Styled message list and message row (Nova look, hover toolbar, grouping, day dividers, "new messages" line, reaction-chip row) | Kit `MessageScroller`, `Message` | M1 |
| G2. Rich text / Markdown view with theme tokens and a plugin hook for inline entities | Kit `TextView` | M1 |
| G3. Chat composer: multi-line, Enter/Shift+Enter policy, formatting shortcuts, paste-image hook, IME composition, caret-anchored inline autocomplete trigger API (`@`, `~`, `:`, `/`) | gpuicn Textarea + Popover (capabilities checked in spike S4) | M1 |
| G4. Command palette / quick switcher | Kit `Command` | M1 |
| G5. Badge, Kbd, Skeleton, Spinner, Tag in Nova styling | Kit equivalents | M1 |
| G6. Attachment card and upload tray | Kit `Attachment` | M1 |
| G7. Emoji picker. M1 release: search, categories, recents, caller-supplied custom emoji. M2 release: skin tone, frequently used, polish | Popover + VirtualList grid | M1 (minimal), M2 |
| G8. Image viewer / lightbox (zoom, pan, keyboard next/previous) | Dialog | M1 |
| G9. Hover card for user profiles | Kit `HoverCard` | M2 |
| G10. Title bar with platform-correct window controls | Kit `TitleBar` | M1 |

**Cross-cutting requirement: images come from Matty.** Avatars
(`users/{id}/image`), custom emoji (`emoji/{id}/image`), file thumbnails and
previews (`files/{id}/thumbnail`, `/preview`) and the image proxy all need the
session token, and GPUI's `img(url)` sends no headers. So every image slot in
G1, G6, G7, G8 and G9 takes an `ImageSource` or decoded image that Matty
supplies from its media cache. gpuicn never fetches over the network.

**What Matty needs from each component.** These are requirements for Arya to
build against; the spikes and integration work will add to them.

- **G1 message list and row:** rows are rendered by the caller. Day dividers
  and the "New messages" line are caller-supplied separator rows with stable
  IDs. The caller decides grouping; G1 offers a "continuation" row variant
  that shows the time in the gutter on hover. Slots for the hover toolbar and
  a reaction-chip row (`on_toggle(emoji)` and a tooltip slot for who
  reacted). Row variants for pending, failed, highlighted (permalink target)
  and mentions-you. A keyboard selection mode (Up/Down plus an `on_key`
  callback for single-key actions). Text selection across rows with copy.
  `prepend` that keeps the anchor row in place (verified by S1). A per-row
  accessible description.
- **G2 rich text:** use Kit's `TextView` plugin hook (`MarkdownPlugin`).
  Minimum: custom link schemes (`matty://user/<id>`, `matty://channel/<id>`)
  rendered with theme token colors and delivered to an `on_link_click`
  callback with the raw href; inline images at text height for `:emoji:`.
  `matty-markdown` rewrites mentions, channels and emoji to those links
  before rendering, so gpuicn needs no custom inline parser.
- **G3 composer:** the caret rectangle in window coordinates; get and set the
  selection range; insert at caret; `on_paste` that can claim image and file
  payloads; `is_composing()` so Enter during IME composition never sends; an
  `on_key` hook that runs before the default so Matty owns Enter,
  Shift+Enter, Up-at-top and Tab; auto-grow up to a maximum height; an
  autocomplete popup anchored to the caret (reusing gpuicn's Autocomplete
  styling where it fits).
- **G7 emoji picker:** `custom: Vec<(name, ImageSource)>`, recents owned by
  the caller, `on_pick(shortcode)`, search starting at two characters as in
  Mattermost.
- **G10 title bar:** Linux draws its own window controls; Windows supports
  snap-layout hit testing on the maximize button; the drag region excludes
  interactive children such as the search box.

What stays in Matty: the Mattermost API client, WebSocket, sync engine, cache,
Markdown flavor rules (which entities exist, how a mention resolves), unread
logic, notification policy, and every screen composition.

### Known risks in the base

- **Variable-height virtualization.** gpuicn's VirtualList is fixed-height and
  its parity doc records that GPUI's `list` clears offscreen height hints on
  width changes. Kit's `MessageScroller` is built on that same `list`. Before
  M1 starts, run a spike (S1, section 11): 50k mixed-height messages, window
  resize, prepend 200 older messages, and check that the anchor message stays
  put and the scrollbar does not jump. Matty runs it against G1 (or Kit's
  scroller directly if G1 is not ready yet) and reports the result to gpuicn;
  any fix belongs in gpuicn or GPUI.
- **Platform qualification.** gpuicn's native evidence is macOS only. Windows
  and Linux need their own qualification passes, which is why they ship one
  milestone later than macOS (section 10).
- **Screen readers.** GPUI exposes roles, but a full VoiceOver/NVDA/Orca
  walkthrough of gpuicn has not been done. Matty's accessibility gate depends on
  it (section 8).

## 3. Scope by milestone

### M0 — Foundations (no user-facing release)

- Workspace, CI, pins, lints, app skeleton window with gpuicn theme and Geist.
- `matty-api`: typed REST v4 client for the endpoints M1 needs; WebSocket
  client with auth, reconnect and event decoding.
- `matty-store`: SQLite cache schema and migrations.
- Spikes S1 (variable-height list), S2 (async runtime bridge), S3 (SSO
  flow and URL scheme), S4 (composer capabilities and IME).
- A local Mattermost server in Docker for development and integration tests.

### M1 — Daily driver, single server

The smallest thing someone can use for a full workday. M1 ships in two steps:
an **alpha** with everything below except the items marked *(beta)*, then a
**beta** that adds them. The alpha is deliberately smaller so dogfooding
starts early.

- **Sign in:** server URL with validation (`/api/v4/system/ping`), email or
  username + password, LDAP/AD (same endpoint, shown when `EnableLdap` is on),
  MFA, GitLab/Google/Office 365/OpenID/SAML through the system browser
  (section 6.6), personal access token as an advanced option. Remember the
  session in the keychain.
- **Finding people and channels:** start a DM or group message from the quick
  switcher or a "+" in the sidebar (`POST /channels/direct`,
  `/channels/group`); browse and join public channels; leave a channel.
- **Navigation:** team switcher, sidebar with the user's server-side
  categories (Favorites, custom categories, Channels, Direct Messages), unread
  bolding, mention badges, muted dimming, collapse per category, "Unreads"
  filter *(beta)*. Quick switcher on Cmd/Ctrl+K.
- **Reading:** channel view with message grouping, day dividers, "New
  messages" line, jump-to-unread, jump-to-latest, infinite scroll into history,
  Markdown (bold, italics, strike, code, code blocks with highlighting, quotes,
  lists, tables, links, headings, inline images), @mentions and ~channels as
  clickable entities, custom and standard emoji, edited markers, system
  messages, image thumbnails, link previews (OpenGraph from the server)
  *(beta)*.
- **Writing:** composer with Enter to send, Shift+Enter for newline (setting to
  invert, as in Mattermost), @, ~ and : autocomplete, IME input (CJK and
  dead keys), per-channel drafts that survive restart and sync with the
  server when it allows synced drafts (`AllowSyncedDrafts`, `/drafts`
  endpoints), Up to edit last message, optimistic send with retry, edit,
  delete with confirmation, reply in thread.
- **Threads:** right-hand thread pane, Collapsed Reply Threads support
  including the "Threads" view with followed threads and unread replies.
- **Reactions:** add from the hover toolbar and the emoji picker (the minimal
  G7 release from gpuicn), toggle by clicking a reaction, who-reacted
  tooltip.
- **Search, saved and pinned:** search with terms, quoted phrases, `from:`
  and `in:`, results in the right pane with jump to message; save and pin a
  message; Saved, Pinned and Recent mentions panes; copy link to a message.
  Full modifier support and file search come in M2.
- **Presence and typing:** status dots, set own status (online, away, DND,
  offline; DND with a duration *(beta)*), "X is typing…".
- **Notifications:** native OS notifications for mentions, DMs and keywords
  (keywords *(beta)*) following the user's server-side notification preferences and per-channel
  overrides; dock/taskbar badge; clicking opens the right message. DND is
  respected.
- **Unread correctness:** switching channels calls
  `channels/members/me/view` with `prev_channel_id`. While a channel stays
  open, Matty marks it viewed again (debounced) when a new post arrives only
  if the window is focused, the channel is visible and the list is at the
  bottom. A user scrolled up into history never has newer messages marked
  read. "Mark as unread" from a message.
- **Files:** upload by picker, drag and drop and paste, with progress; download
  to the user's Downloads folder with "Show in folder"; image lightbox.
- **Settings:** theme (system/light/dark), message display (standard/compact),
  send-on-Enter behavior, notification preferences mirrored from the server,
  sign out.
- **Resilience:** offline banner, cached channels readable offline, failed
  sends kept with Retry, reconnect with backfill. A persistent outbox for
  messages written while offline is *(beta)*.
- **Platforms:** macOS (Apple silicon and Intel), signed and notarized, with
  auto-update.

### M2 — Parity with the official desktop app for most people

- Multiple servers with a server rail (Mattermost desktop and Discord pattern),
  per-server notifications and badges.
- Full search modifiers (`before:`, `after:`, `on:`, `-` exclusions,
  wildcards) and file search.
- Channel management: create public/private, rename, header
  and purpose, members list, add members, mute, favorite, move to category,
  create and reorder sidebar categories, archive.
- Emoji picker polish (G7 M2 release), custom emoji upload.
- Slash commands: built-in and server-registered commands with autocomplete;
  `/commands/execute`.
- Interactive message attachments (buttons and menus from integrations) and
  ephemeral messages.
- Profile hover cards, user profile pane, edit own profile and avatar.
- Message actions: copy text, forward, follow/unfollow thread, set
  reminder, mark unread.
- Windows and Linux builds with the same feature set (after qualification).
- Accessibility pass with a real screen reader on each platform.

### M3 — Power and polish

- Message formatting toolbar (optional, collapsed by default), Markdown
  preview.
- Scheduled messages, message priority and acknowledgements, persistent
  notifications (where the server licenses them).
- Custom status with emoji and expiry, timezone display.
- Split view (two channels side by side) and pop-out windows, which GPUI makes
  cheap and which Electron clients do poorly.
- Global search across servers, keyboard-only triage mode ("next unread" with
  Alt+Shift+Up/Down), unread digest view.
- Spell check (OS spell checker where available).
- Localization of the UI.
- Mattermost Apps framework bindings and channel bookmarks.

### Not planned, with a fallback

- **Webapp plugins** (Boards, Playbooks, third-party UI plugins) run JavaScript
  inside the web app and cannot run in a native client. Post types we do not
  render show a compact card with "Open in browser".
- **Calls** needs WebRTC and a plugin protocol; revisit after M3 only if users
  ask. Show incoming call posts with an "Open in browser" action.
- **Admin console and System Console.** Link out to the web app.

## 4. UX specification

### 4.1 Layout

```
┌──────────────────────────────────────────────────────────────────────────┐
│ ● ● ●   ‹ ›   [ Search  ⌘K / ⌘F                 ]            ⓘ  (avatar) │  title bar
├────┬───────────────┬─────────────────────────────────┬───────────────────┤
│ S1 │ Team name   ▾ │ # design   ☆  👤 12  📌 3  …     │ Thread        ✕   │
│ S2 │ ⊕ Threads     │ header text                     │                   │
│ S3 │ ⊕ Drafts      ├─────────────────────────────────┤ root message      │
│    │ ▾ FAVORITES   │ ─────── Today ───────           │ 4 replies         │
│ +  │   # design  2 │ (av) Ana  10:42                 │ ...               │
│    │ ▾ CHANNELS    │      message text               │                   │
│    │   # general   │      👍 3  🎉 1                  │                   │
│    │   # random    │ ───── New messages ─────        │                   │
│    │ ▾ DIRECT MSGS │ (av) Ben  10:44                 │                   │
│    │   ● Ana       │      ...                        │                   │
│    │               ├─────────────────────────────────┤───────────────────┤
│    │  (me) ● ▾     │ [ composer                 ]  ➤ │ [ reply      ]  ➤ │
└────┴───────────────┴─────────────────────────────────┴───────────────────┘
 rail   sidebar         channel                           right pane
```

- **Rail**: 64px. In M1 it appears when the user belongs to more than one
  team and shows team icons with unread dots and mention counts, as
  Mattermost does. In M2 it also holds servers (server icons above, the
  current server's teams below), with drag to reorder.
- **Sidebar**: 240px default, resizable 200–400px, collapsible with
  Cmd/Ctrl+Shift+D (Slack's binding). Team name at the top, then Threads and
  Drafts entries, then the user's categories. Own avatar and status
  at the bottom.
- **Channel view**: header (name, favorite, member count, pinned count,
  channel info), message list, composer. Composer grows to ~40% of height, then
  scrolls.
- **Right pane**: threads, search results, saved, pinned, mentions, channel
  info and members. Resizable 320–640px, Escape closes. Width persists.
- **Narrow windows**: below ~900px the right pane overlays the channel view as
  a drawer; below ~640px the sidebar collapses to an overlay too. The window
  minimum is 480×400.

### 4.2 Message list behavior

These are where chat clients feel right or wrong. Each one gets a regression
test.

- Opening a channel lands on the first unread message with the "New
  messages" line visible, or at the bottom when there are no unreads.
- When at the bottom, new messages stick to the bottom. When scrolled up, new
  messages do not move the viewport; a "Jump to latest (N new)" pill appears.
- Loading older history keeps the anchor message at the same screen position.
- Images and embeds reserve space using the dimensions in `post.metadata` so the
  list does not jump when they load.
- Messages from the same sender within 5 minutes collapse under one header;
  hovering a collapsed message shows its time in the gutter.
- Hovering a message shows a floating action bar (react, reply, more) at the
  top right, as in Slack and Discord. Every hover action is also reachable by
  keyboard and context menu.
- Keyboard message selection: the user enters it explicitly (Shift+Tab from
  the composer, or clicking a message's gutter). While a message is selected, Up/Down move the selection
  and single keys act on it (R reply, E edit, Delete removes own, + react,
  P pin, S save, U mark unread), as in Discord's keyboard message
  navigation. Escape returns to the composer. When no message is selected,
  any printable key focuses the composer and types into it (section 4.4).
  Single keys therefore never steal ordinary typing.
- Text selection works across messages and copies as plain text, with a
  "Copy as Markdown" option in the context menu.
- Pending messages show at reduced opacity; failed ones show a red marker
  with Retry and Delete.

### 4.3 Composer

- Enter sends, Shift+Enter inserts a newline. Ctrl/Cmd+Enter sends when the
  "send on Ctrl+Enter" preference is on (Mattermost's setting, mirrored).
- Typing `@`, `~`, `:` (after two characters) or `/` at the start opens an
  inline autocomplete above the composer; Tab or Enter accepts, Escape closes.
- Up in an empty composer edits your last message in place.
- Cmd/Ctrl+B (bold), Cmd/Ctrl+I (italic) and Cmd/Ctrl+Alt+K (link), which are
  Mattermost's bindings, plus Cmd/Ctrl+Shift+X (strike) and Cmd/Ctrl+Shift+C
  (code) from Slack, wrap the selection in Markdown. Cmd/Ctrl+K stays the
  quick switcher everywhere.
- Paste of an image or file adds an attachment chip; drop on the window does
  the same for the current channel.
- Drafts persist per channel and thread, and appear in the Drafts view with a
  pencil icon in the sidebar.
- Message length is checked against the server's `MaxPostSize` with a counter
  near the limit.

### 4.4 Keyboard shortcuts

Use Mattermost's published shortcuts so muscle memory transfers. Cmd on macOS,
Ctrl elsewhere. Bindings marked (MM) are Mattermost's; the others are Slack
conventions or Matty additions. A Cmd/Ctrl+/ sheet lists them all.

| Action | Shortcut |
| --- | --- |
| Quick switcher | Cmd+K (MM) |
| Direct messages menu | Cmd+Shift+K (MM) |
| Search | Cmd+Shift+F (MM); Cmd+F also focuses search when not in a text field |
| Previous / next channel | Alt+Up / Alt+Down (MM) |
| Previous / next unread channel | Alt+Shift+Up / Alt+Shift+Down (MM) |
| Back / forward in history | Cmd+[ / Cmd+] on macOS, Alt+Left / Alt+Right on Windows and Linux (MM) |
| Switch server n (M2) | Cmd+1…9 |
| Unreads filter | Cmd+Shift+U (MM) |
| Channel info | Cmd+Shift+I (MM) |
| Toggle / expand right pane | Cmd+. / Cmd+Shift+. (MM) |
| Recent mentions | Cmd+Shift+M (MM) |
| Upload files | Cmd+U (MM) |
| Edit last message | Up in empty composer (MM) |
| Reply to last message | Shift+Up in empty composer (MM) |
| React to last message | Cmd+Shift+\ (MM) |
| Focus composer | Cmd+Shift+L (MM), or just start typing |
| Mark channel read | Escape in the channel (Matty addition, from Slack) |
| Mark all read | Shift+Escape (Matty addition, from Slack) |
| Toggle sidebar | Cmd+Shift+D (Slack) |
| Close right pane | Escape |
| Zoom in / out / reset | Cmd+= / Cmd+- / Cmd+0 |
| Preferences | Cmd+, |

"Just start typing" focuses the composer when focus is anywhere in the
channel view and no message is selected, as in Slack and Telegram.
Alt+Up/Down switch channels everywhere except inside a multi-line composer
draft on macOS, where they keep their text-navigation meaning. Before M1,
recheck the table against Mattermost's current shortcut page and each OS's
reserved shortcuts.

### 4.5 Onboarding and empty, loading and error states

- First launch: a single centered card ("Connect to your Mattermost server"),
  URL field with `https://` assumed, inline validation, then the sign-in
  methods the server advertises from `/api/v4/config/client` (only show SSO
  buttons that are enabled).
- Loading: skeleton rows for sidebar and messages, never a spinner over a
  blank window. Cached content renders immediately.
- Empty channel: friendly Empty state with "Add members" and the channel's
  purpose.
- Errors: inline and specific ("This server needs Mattermost 9.x or later", using the minimum version
  set in section 6.3,
  "Your session expired, sign in again") with a single action. Network errors
  become the offline banner, not dialogs.
- Destructive actions (delete message, leave private channel, sign out) use
  AlertDialog with the action named in the button.

### 4.6 Notifications

- Follow the server preferences exactly: desktop notifications for all, mentions
  or nothing; per-channel overrides; keywords; thread reply notifications for
  followed threads; DND and its schedule.
- Do not notify for the channel that is visible in a focused window.
- Group by channel on platforms that support it. Notification text respects
  the "show message preview" preference.
- Dock badge shows mention count; the tray (Windows, Linux) shows a dot for
  unreads and a count for mentions.
- Sounds: one subtle default, selectable, and off by default for non-mentions.
- Clicking a notification focuses the window, switches server/team/channel and
  highlights the message for two seconds.

## 5. Design references and visual direction

### What to take from each reference

| Reference | Take | Leave |
| --- | --- | --- |
| **Slack** | Sidebar sections and custom categories, thread pane on the right, hover action bar, Cmd+K switcher, "Unreads" view, composer with inline formatting | Heavy purple chrome, Huddles/Canvas clutter, slow switches |
| **Mattermost desktop / webapp** | Server tabs, team sidebar, Collapsed Reply Threads view, shortcut set, notification preference model, so existing users feel at home | Electron startup, dense settings modal, visual inconsistency between plugins |
| **Discord** | Server rail with unread pills, message grouping and gutter timestamps, keyboard message actions, reaction chips, jump-to-present pill | Gamer aesthetic, nitro upsells |
| **Telegram Desktop** | Speed as a feature: instant channel switch, smooth scroll, tiny memory; tidy media viewer; chat folders as a later idea for categories | Bubble layout for group chat (left-aligned rows read better in team chat) |
| **Linear / Zed** | Restraint: neutral surfaces, one accent, crisp 1px borders, keyboard-first, command palette everywhere | — |

Collect screenshots of each pattern above into `docs/design/references/`
during M0, annotated with what we copy. Build the Matty screens as a Figma
file (or equivalent) from gpuicn tokens before M1 UI work, covering light,
dark, compact and the narrow layouts.

### Visual direction: "quiet, fast, native"

- **Base:** gpuicn Nova Neutral, so Matty looks like gpuicn and gets its theme
  machinery for free. Surfaces: window background, a 2% darker sidebar, and the
  content area. Separation by 1px borders, not shadows. Shadows only on
  overlays.
- **Accent:** one brand color for selection, focus ring, links, mention
  highlight and the primary button. Proposal: a cool indigo–blue
  (`#3b5bdb`-ish in light, lifted for dark), tuned for 4.5:1 text contrast.
  Mentions of you get a soft accent-tinted row background and an accent bar in
  the gutter, as in Slack and Discord.
- **Type:** Geist (already shipped by `gpuicn new`) at 14px for messages and
  13px for chrome, line height 1.45 for messages. Geist Mono for code. Bold
  sender names, muted timestamps. Respect the app text scale setting.
- **Density:** two message displays, "Standard" (avatars, 8px gaps) and
  "Compact" (no avatars, name inline, like IRC/Mattermost compact). gpuicn's
  spacing unit makes a density preference nearly free.
- **Motion:** 120–180ms, ease-out, only for overlays, pane open and close, and
  the jump-to-latest pill. No motion in the message list. Reduced motion
  honored from the OS (Matty wires it, since GPUI does not).
- **Icons:** Lucide via gpui-icons at 16px in chrome, 20px in the rail; stroke
  width consistent; every icon-only button has a tooltip and accessible label.
- **Themes:** light, dark, follow system. M2 imports Mattermost's own custom
  theme JSON (the 20-color theme users share) and maps it onto gpuicn tokens,
  so teams with a corporate theme keep it.
- **App icon:** a simple geometric mark distinct from Mattermost's logo (we
  must not use their trademark); commission or design in M0.

## 6. Architecture

### 6.1 Crates

```
matty/
  crates/
    matty-api      REST v4 + WebSocket client, typed models (no GPUI)
    matty-store    SQLite cache, migrations, queries (no GPUI)
    matty-sync     Sync engine: merges REST + WS into the store, emits change events (no GPUI)
    matty-markdown Mattermost Markdown flavor → AST used by the view (no GPUI)
    matty-notify   OS notifications, badges, sounds per platform
    matty-app      GPUI app: windows, views, actions, keymap, settings
    matty          binary
  docs/  scripts/  packaging/
```

Everything below `matty-app` is plain Rust and testable without a window.

### 6.2 Runtime and threading

GPUI owns the main thread and has its own foreground and background
executors. HTTP and WebSocket libraries (`reqwest`, `tokio-tungstenite`)
expect tokio. Spike S2 picks between:

- **A (recommended start):** one dedicated tokio runtime on a background
  thread for all network I/O; communication with GPUI through channels
  (`flume` or `async-channel`). Decoding and store writes happen off the main
  thread. The UI only receives small "changed" events and reads from the store
  or in-memory models.
- **B:** a smol-compatible stack (`async-tungstenite` + an HTTP client that
  runs on GPUI's executor). Fewer moving parts, less mature.

Rule either way: no network or disk I/O on the main thread, and no frame
waits on a lock held by the network thread. The SQLite connection lives on
the sync side only. The sync engine publishes immutable, `Arc`-shared
snapshots (channel list, unread counts, a window of posts per open channel)
to GPUI entities, and views render from those. The main thread never opens
the database.

### 6.3 Mattermost API usage

- **Version support:** current Mattermost release and the last two ESRs, and
  never older than the release that added `POST /users/login/desktop_token`
  (the 9.x line; spike S3 pins the exact version).
  Read `/api/v4/config/client?format=old` and
  `/api/v4/license/client?format=old` at
  connect time and feature-gate on them (Collapsed Reply Threads, message
  priority, custom emoji, etc.).
- **Initial load:** `users/me`, `users/me/teams`, `users/me/teams/members`,
  `users/me/teams/{team}/channels` and channel members, sidebar categories
  (`users/me/teams/{team}/channels/categories`), preferences, custom emoji
  list (paged), and statuses for visible DMs. Then posts for the open channel
  only.
- **Posts:** `users/{uid}/channels/{cid}/posts/unread?limit_before&limit_after`
  to open a channel at its first unread; `channels/{id}/posts` with
  `before`/`after`/`per_page` for paging, `collapsedThreads=true` for
  root-only lists under CRT, and `since` for backfill. Threads via
  `posts/{id}/thread` (with `fromCreateAt`, `direction` and `perPage` for long
  threads) and the CRT endpoints (`users/{id}/teams/{team}/threads`,
  `PUT .../threads/{thread}/read/{timestamp}` to mark a thread read).
- **Unread math:** with CRT on, channel unread and mention counts use
  `total_msg_count_root`, `msg_count_root` and `mention_count_root`; with it
  off, the non-root fields. Team badges come from `users/{uid}/teams/unread`.
  The sync tests cover both modes.
- **Users:** fetch lazily by IDs (`users/ids`) and, for mentions, by
  username (`users/usernames`) as they appear; statuses in batches with
  `POST users/status/ids`; cache with `last_picture_update` for avatars
  (`users/{id}/image`).
- **Writes:** posts, patches, deletes, reactions, `channels/members/me/view`
  for marking read, `users/{id}/posts/{post}/set_unread` for mark unread,
  `files` for upload with progress, typing over the WebSocket.
- **M1 feature endpoints:** search and recent mentions with
  `POST teams/{tid}/posts/search` (`terms`, `is_or_search`,
  `time_zone_offset`, `page`, `per_page`; mentions use the user's mention
  keys); saved messages with `users/{uid}/posts/flagged` and the
  `flagged_post` preference category; pinned with `channels/{cid}/pinned`
  and `POST posts/{id}/pin` / `unpin`; header counts from
  `channels/{cid}/stats`; browse with `teams/{tid}/channels` and
  `.../channels/search`; join with `POST channels/{cid}/members`; leave with
  `DELETE channels/{cid}/members/{uid}`; reactions with `POST reactions` and
  `DELETE users/{uid}/posts/{pid}/reactions/{emoji}`; status with
  `PUT users/{uid}/status` (`dnd_end_time` for DND with a duration); edit with
  `PUT posts/{id}/patch`; delete with `DELETE posts/{id}`; permalinks with
  `GET posts/{id}` then `channels/{cid}`. This list is the M0 scope for
  `matty-api`.
- **Login details:** `POST /users/login` with
  `{login_id, password, token, device_id}`, plus `ldap_only: true` when the
  user picked the LDAP method; an MFA requirement is detected by
  the error id `mfa.validate_token.authenticate.app_error`, not by the HTTP
  status.
- **Rate limits:** respect `X-RateLimit-*` and back off on 429.

### 6.4 WebSocket and sync

- Connect to `/api/v4/websocket`, authenticate with the token
  (`authentication_challenge`), handle `hello`.
- Track the last `seq` and the `connection_id` from `hello`. Resume by
  reconnecting to `/api/v4/websocket?connection_id=…&sequence_number=…`. If the
  `hello` that follows carries a different `connection_id`, the server could
  not replay (reliable WebSockets off, buffer expired, or server restarted),
  and a full REST resync is mandatory: `users/{uid}/teams/{tid}/channels/members`
  for unreads, `posts?since=` for channels with cached posts (newest visible
  first), and `threads?since=` under CRT. `since` always uses the last server
  timestamp seen, never the local clock. A `since` response includes deleted
  posts (`delete_at` set), which are applied as deletes. `posts?since=` is not
  paged, so when the gap is long (default: over 6 hours) or the response
  passes a size cap, Matty drops that channel's cached posts and reloads the
  latest page instead of replaying everything.
- Send `{"action":"ping"}` about every 30s as the webapp does, answer
  server pings, and run a client-side liveness timeout so a silently
  dead socket is noticed within ~30s.
- Reconnect with jittered exponential backoff, capped at 30s, and immediately
  on OS network-change and wake-from-sleep signals.
- Handle at least: `posted`, `post_edited`, `post_deleted`, `reaction_added`,
  `reaction_removed`, `typing`, `status_change`, `channel_viewed`,
  `multiple_channels_viewed`, `channel_created/updated/deleted`,
  `user_added/removed`, `user_updated`, `new_user`, `user_role_updated`,
  `preference(s)_changed`, `preferences_deleted`, `channel_restored`,
  `channel_scheme_updated` (permissions change which actions are offered),
  `update_team`, `delete_team`,
  `sidebar_category_*`, `thread_updated`, `thread_read_changed`,
  `thread_follow_changed`, `emoji_added`, `config_changed`,
  `license_changed`, `ephemeral_message`, `post_unread`,
  `channel_member_updated`, `direct_added`, `group_added`, `added_to_team`,
  `leave_team`, `channel_converted`, `draft_created`, `draft_updated`,
  `draft_deleted`, and in M3 `post_acknowledgement_added/removed`.
  `channel_viewed` is deprecated in favor of `multiple_channels_viewed`, so
  handle both. `posted` and `post_edited` carry the post as a JSON string
  inside `data.post`, decoded a second time.
- The sync engine is the only writer to the store. It applies each event as a
  transaction and emits a typed change set (channel X posts changed, unread
  counts changed). Views subscribe to the change sets they care about.

### 6.5 State and the cache

- **SQLite** (`rusqlite`, WAL mode), one database per server account, in the
  OS app data directory. Tables: teams, channels, channel_members, categories,
  users, posts (with root_id, reply_count), reactions, files metadata, threads,
  drafts, outbox, preferences, emoji, sync cursors.
- **What is cached:** everything needed to render the sidebar and the last
  ~200 messages of each recently viewed channel. Older history is fetched on
  demand and evicted by LRU with a size cap (default 500MB including media).
- **Media cache:** avatars, thumbnails, previews on disk keyed by file ID and
  update time.
- **In-memory models:** GPUI entities for the open window state (selected
  server, team, channel, right pane) holding the snapshots the sync engine
  publishes (section 6.2). Message row elements are built only for the
  visible range.
- **Outbox:** each send gets a client `pending_post_id`; the server echoes
  it, so the optimistic row is replaced, not duplicated, whether the REST
  response or the WebSocket `posted` event arrives first. In the M1 beta,
  messages written offline are stored and sent in order on reconnect.

### 6.6 Authentication and security

- Password login: `POST /users/login`, token from the `Token` header. MFA
  prompt when the server asks for it.
- SSO uses the server's desktop flow, the same one the official desktop app
  uses with an external browser. Matty generates a random `client_token` and
  opens the system browser at `{server}/oauth/{service}/login?desktop_token=…`
  (or `/login/sso/saml?desktop_token=…`). After sign-in the server sends the
  browser to `{server}/login/desktop`, which hands off to the
  `mattermost://` URL scheme with a short-lived `server_token` (about three
  minutes). Matty then calls `POST /api/v4/users/login/desktop_token` with the
  token and its device id to get a session. (The mobile flow does not work
  for us: it only redirects to schemes an admin lists in
  `NativeAppSettings.AppCustomURLSchemes`, and SAML expects a mobile user
  agent.)
- That hand-off means Matty must handle `mattermost://` links, a scheme the
  official app also registers. Spike S3 tests on each OS: (a) registering
  `mattermost://` next to the official app and what happens when both are
  installed (which one receives the callback, and whether Matty can ask to be
  the default), (b) the `AppCustomURLSchemes` route when an admin adds
  `matty://`, and (c) the personal access token fallback. Development builds
  use `dev-` prefixed tokens, which the server routes to `mattermost-dev://`,
  so dev and release builds never fight over the scheme.
- Store tokens in the OS keychain: the `keyring` crate (Keychain, Credential
  Manager, Secret Service), and `oo7` under Flatpak, which speaks the Secret
  portal. Linux often has
  no secret service running (minimal desktops, WSL); then fall back to a
  file encrypted with a key derived from a user-chosen passphrase, with a
  visible warning in settings, rather than failing sign-in. Never store a
  token in plain text, never log it.
- TLS through rustls with the OS trust store (`rustls-platform-verifier`) so
  corporate CAs work. Show a clear error for certificate problems; no "ignore
  certificate" button in M1.
- Links open in the system browser. Link previews and image dimensions come
  from `post.metadata` (`embeds` of type `opengraph`, `image`, `link` and
  `message_attachment`, `images`, and `files[].width/height`), gated by
  `EnableLinkPreviews`; the `/opengraph` endpoint is only for previews while
  composing. Inline Markdown images and preview thumbnails go through
  the server's image proxy when it is enabled. When it is not (the common
  case), Matty does what the webapp does and loads them directly, so images
  never look broken; a privacy setting "Load remote images: always / ask"
  switches to a click-to-load placeholder.
- Logs are local, rotate, and redact tokens, message text and file names by
  default. Crash reports are opt-in.

### 6.7 Notifications, badges and OS integration

- macOS: `UNUserNotificationCenter` (via `objc2` bindings) with app identity
  from the signed bundle; dock badge. It silently does nothing for an unsigned
  binary, so development builds run from an ad-hoc signed `.app` with a dev
  bundle id.
- Windows: toast notifications through the WinRT APIs (`windows` crate).
  For a non-MSIX install the AppUserModelID must be on the Start Menu shortcut
  the installer creates and also set at startup with
  `SetCurrentProcessExplicitAppUserModelID`; taskbar overlay badge.
- Acceptance evidence for notifications is a real click-through on a signed
  build on each platform, not a unit test.
- Linux: freedesktop notifications over D-Bus (`notify-rust`); badge via the
  Unity launcher API where supported, otherwise tray only.
- Deep links: Matty registers `matty://` for its own links and handles
  `mattermost://` as needed for the SSO hand-off (section 6.6, spike S3).
  Permalinks (`https://server/team/pl/postid`) pasted or clicked inside the
  app open in place.
- Start at login (optional), single instance (second launch focuses the
  existing window and forwards any deep link), and system tray on Windows and
  Linux.

### 6.8 Markdown

`matty-markdown` parses Mattermost's flavor (CommonMark plus GFM tables and
strikethrough, autolinks, @mentions, ~channels, :emoji:, hashtags, LaTeX and
inline LaTeX as code blocks until there is a renderer) into an AST the view
renders through the gpuicn rich text view (G2). Keeping the parser out of GPUI
makes it testable with fixtures taken from the Mattermost webapp's own tests.

## 7. Settings

Keep server-backed preferences on the server (they sync to the user's other
clients) and app preferences locally.

| Server-synced | Local to Matty |
| --- | --- |
| Display name format, clock (12/24h), message display (standard/compact), collapsed reply threads, send on Ctrl+Enter, notification preferences, theme (the `theme` preference category, used when importing Mattermost themes), timezone | Window size and pane widths, launch at login, tray behavior, text scale, cache size, download folder, update channel (stable/beta), log level |

## 8. Accessibility and internationalization

- Every control has an accessible name and role; the message list uses the
  `Log` role with each message announced as "sender, time, text, N reactions,
  N replies".
- Full keyboard operation of every flow, checked by a keyboard-only journey
  per milestone. Visible focus ring always.
- Contrast at least 4.5:1 for text and 3:1 for UI in both themes and in
  imported themes (warn when an imported theme fails).
- Respect OS text scaling, high contrast where the platform exposes it, and
  reduced motion.
- Screen reader passes per platform: VoiceOver (M1 for macOS), NVDA and
  Narrator (M2), Orca (M2). Anything GPUI cannot expose is recorded as an
  upstream blocker, not hidden.
- Strings go through a catalog from day one (Fluent or `rust-i18n`) even if
  only English ships until M3. Dates and times use the user's locale and
  Mattermost timezone setting. Right-to-left text renders correctly inside
  messages from M1; full RTL layout is M3.

## 9. Performance budgets

Measured on a named reference machine per platform (for example an M1 MacBook
Air, a mid-range Windows laptop, and a Linux laptop) and recorded with the
gpuicn performance harness approach. Proposed targets, to be agreed after the
M0 baseline:

| Metric | Target |
| --- | --- |
| Cold start to cached sidebar visible | < 500ms |
| Channel switch (cached) to first paint | < 50ms |
| Channel switch (uncached, good network) | < 300ms to messages |
| Scrolling 10k-message channel | no missed frames at 60Hz; p99 frame < 12ms |
| Keypress to glyph in composer | < 16ms |
| Idle memory, 1 server, 50 channels | < 150MB |
| Idle CPU with WebSocket connected | ~0%, no idle frames |
| Install size | < 40MB, including the embedded Lucide SVGs and Geist fonts (the gpuicn showcase build is the baseline) |

Each is checked in CI where possible (headless timings for store and sync)
and manually on the reference machines before each release.

## 10. Packaging, distribution and updates

- **macOS:** universal `.app` in a signed, notarized DMG; Developer ID
  certificate; hardened runtime; `matty://` URL scheme in Info.plist.
- **Windows (M2):** signed MSI or MSIX via `cargo-packager` or WiX;
  AppUserModelID for notifications; winget manifest.
- **Linux (M2):** AppImage and Flatpak (Flathub), `.deb` and `.rpm`; desktop
  file with URL handler; AUR package by the community.
- **Homebrew cask** for macOS from M1 beta, marked `auto_updates true`; the
  in-app updater remains the single update path so the two never fight.
- **Auto-update:** an update feed hosted on GitHub Releases, with signed
  update manifests (`cargo-packager-updater` or Sparkle on macOS), a stable and
  a beta channel, and a "restart to update" prompt that never interrupts
  typing. Flatpak and package managers update themselves.
- **Release process:** tag → CI builds all platforms → signs → uploads
  artifacts and checksums → updates feeds. Releases need Arya's explicit go,
  as with gpuicn.
- **Licensing and trademarks:** MIT like gpuicn. Do not use the Mattermost
  logo; describe Matty as "an unofficial client for Mattermost".

## 11. Testing strategy

- **Unit tests:** API model decoding against recorded JSON fixtures for each
  supported server version; Markdown parser fixtures; store migrations; sync
  engine event application (including out-of-order, duplicate and missed
  events). Property tests for reconnect with duplicate or out-of-order `seq`,
  and for `pending_post_id` dedupe when the WebSocket `posted` event arrives
  before the REST response.
- **Integration tests:** a Mattermost server in Docker Compose
  (`mattermost/mattermost-team-edition` or `mattermost-enterprise-edition`
  without a license, plus Postgres) in CI, seeded by a script with teams, channels, users, posts and threads.
  Tests drive `matty-sync` against it: login, initial load, send/edit/delete,
  reactions, reconnect with missed events, CRT.
- **View tests:** GPUI `test-support` for view logic (unread line placement,
  scroll anchoring, composer key handling, focus return).
- **Native interaction checks:** a written journey checklist per milestone run
  by a person on each platform (gpuicn's existing evidence standard),
  including a keyboard-only journey, an IME journey and a screen reader
  journey.
- **Performance:** a seeded large server (50 channels, 100k posts, 5k users)
  and a benchmark script that records the budget metrics in section 9.
- **Dogfooding:** run Matty against a real team server daily from M1 alpha;
  file every paper cut.
- **Spikes before M1:**
  - S1 variable-height list (section 2 risks).
  - S2 async runtime bridge with 1k events/second without frame drops.
  - S3 SSO desktop-token flow on GitLab, OpenID and SAML test
    configurations, including the `mattermost://` coexistence tests in
    section 6.6.
  - S4 composer capabilities: does gpuicn Textarea expose the caret's pixel
    position (to anchor autocomplete), selection ranges (formatting wrap),
    paste interception for images and files, and correct IME composition for
    Japanese, Chinese and Korean input and dead keys? Findings go to Arya as
    requirements for G3.

## 12. Milestone acceptance

| Milestone | Done when |
| --- | --- |
| M0 | Spikes S1–S4 pass or have an agreed workaround; API and sync crates pass integration tests against the Docker server; the window shows a cached sidebar from the store; performance baseline recorded; design file with core screens approved by Arya |
| M1 alpha | Every M1 item not marked *(beta)* works end to end on macOS against the current Mattermost release; keyboard-only, IME and VoiceOver journeys pass; G1–G8 and G10 are available in gpuicn (merged commit pinned); budgets met or exceptions agreed; signed, notarized build auto-updates from the beta feed; one week of dogfooding without data loss or wrong unread state |
| M1 beta | Every *(beta)* item works; the gpuicn gaps are in a tagged gpuicn release and Matty pins it instead of a commit; no known crash; tested against the last two ESRs; Homebrew cask |
| M2 | Every M2 item works on macOS, Windows and Linux with per-platform evidence; screen reader passes on each; installers on all three |
| M3 | Each item ships on its own with the same per-platform evidence as M2, behind a setting where it changes existing behavior |

## 13. Open questions for Arya

1. **Target servers:** is there a specific Mattermost server (version, SSO
   provider, license tier) that must work first? It decides the S3 test matrix
   and which features to gate.
2. **Platform order:** macOS first, Windows and Linux in M2, as proposed?
3. **Accent color and name/icon:** keep Nova neutral with a blue accent, or a
   distinct brand?
4. **Distribution:** is there an Apple Developer ID and a Windows code
   signing certificate available, or should the plan budget for them?
5. **gpuicn sequencing:** which of G1–G10 land first? Matty's M0 can start on
   the API, sync and store crates without any of them; M1 screen work starts
   with G1–G5 and G10.

## Review history

- 28 October 2026: first draft reviewed once by a separate Claude reviewer
  (Fable). Folded in: the SSO desktop-token flow and `mattermost://` scheme
  question, M1 now includes search/saved/pinned, starting DMs and joining
  channels, an alpha/beta split of M1, gpuicn pinning rules, snapshot-based
  main-thread reads, precise WebSocket resume and resync, missing events and
  endpoints, CRT unread math, LDAP and MFA details, synced drafts, IME and
  composer spike S4, remote image policy, Linux keyring fallback, platform
  notification identity requirements, Docker image choice and extra sync
  property tests.
- 29 October 2026: second review (Fable 5.1). It confirmed 20 of the 23
  earlier findings were fully addressed and fixed the remaining three (the
  Alt+Up selection gesture, the Flatpak keyring backend, shortcut
  attribution). New: gpuicn components are Arya's work and Matty states
  precise per-component requirements; all image slots take images from
  Matty; bounded resync after long gaps; the full M1 endpoint list; more
  WebSocket events; a precise mark-read rule; the M1 team rail; Mattermost's
  actual shortcut set; link previews from `post.metadata`.
