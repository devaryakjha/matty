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
| `primary` / `primary-foreground` | `#3056d3` / `#ffffff` | `#6688f2` / `#0f1114` | Buttons, links, selected rail item, mention badges |
| `accent` / `accent-foreground` | `#eef0f4` / `#1b1f2a` | `#242830` / `#e8eaef` | Hover on buttons and menu items |
| `muted` / `muted-foreground` | `#f1f2f4` / `#5f6775` | `#262a33` / `#8b93a5` | Timestamps, placeholders, meta; avatar and icon frames |
| `card` / `popover` | `#ffffff` | `#1c1f26` | Popovers, menus, dialogs |
| `destructive` | `#d42020` | `#f0564f` | Errors, delete, DND |
| `ring` | `#3056d3` | `#6688f2` | Focus |
| `radius` | 8px | 8px | Base; buttons use 6px, inputs 8px, cards 10px |

Every text token reaches 4.5:1 on every surface it sits on, and every
presence color 3:1 (PLAN §8); `scripts/contrast.py` checks the CSS. In
dark mode `primary` is a light blue with near-black text on it, because no
mid-blue can carry white text and still read as a link on `#15171c`.
Hairline borders are decorative and exempt; inputs are told apart by their
`input` fill and placeholder, not their border.

Semantic colors Matty adds on top (`style::Palette`):

| Name | Light | Dark | Used for |
| --- | --- | --- | --- |
| `online` | `#16a34a` | `#3fb950` | Presence dot |
| `away` | `#cc7a00` | `#f2b038` | Presence dot, favorite star |
| `dnd` | `#d42020` | `#f0564f` | Presence dot |

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
  56      240–400              ≥ 260 (flex)                  320–640
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
- **Right pane** (default 400px, 320 to 640) shares the header height and
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
- **Threads view** (PLAN §4.4) replaces the channel when the sidebar's
  Threads row is chosen: a 360px list on the left and the selected thread
  on the right, drawn by the same view as the right pane. The header holds
  "Threads", a joined All / Unread toggle (Unread carries its count) and
  Mark all as read when anything is unread. Rows are avatar, author,
  "in #channel", time; a two-line plain-text snippet of the root in
  `muted-foreground`; then the participants' small avatars and "2 new
  replies" in `primary` or "3 replies" in `meta`, plus a `primary` dot at
  the right while unread. Selecting a row reads it. The empty list says how
  threads get there.
- **Pinned, Saved and Recent mentions** are right-pane listings, opened
  from the channel header's pin count, the title bar's bookmark and @
  buttons, or Cmd/Ctrl+Shift+B and M. Each post sits under a `meta`
  caption "#channel · Today" with a Jump link on the right, drawn as an
  ordinary message (author, time, body, reactions, no hover toolbar), and
  a hairline separates entries. Empty listings say how a post gets there.
- **Drafts view** replaces the channel when the sidebar's Drafts row is
  chosen: a header with the unsent count, then one `card` per draft at
  most 760px wide: where it goes ("#design" or "Reply in #general" with
  the matching icon) and when it was last edited, the text rendered as it
  will read, and a row with Delete in `destructive` on the left and
  Open (outline) and Send (primary) on the right. Deleting or sending
  removes the card at once and the sidebar count follows.
- **Profile card** (PLAN G9; M2 in §3) opens from any avatar in the transcript, in
  a `popover_popup` anchored to it (`design-17`). It leads with the `Lg`
  avatar carrying the presence dot, the display name in `title`,
  `@username` in `meta`, then the position; below, the presence as a
  colored dot with the custom status ("Online · In a review until 3") and
  the local time with a clock glyph. Two half-width buttons close it:
  Message (primary) opens the direct message, Mention (outline) inserts
  `@username ` into the composer and focuses it. Nothing else lives on the
  card; everything about a person beyond this belongs in the right pane.
- **Search** (PLAN §3 M1 "Search, saved and pinned") starts from the quick
  switcher: any typed query adds a last row, "Search messages for …", and
  Cmd+Shift+F opens the pane with an empty field (`design-21`). The right
  pane shows the query in an `input` with a one-line hint for `from:`,
  `in:` and quoted phrases under it, then "N results for …" and the same
  listing as Saved and Mentions (channel and day caption, Jump). While the
  server answers it says "Searching for …"; no match shows an `Empty`
  with the hint again. Cmd+F returns to the field without losing the
  results.
- **Emoji picker** (PLAN §3 M1 reactions, gpuicn G7) is gpuicn's
  `EmojiPicker` in a popover on the composer's emoji button, borderless on
  a `popover`-colored surface (`design-20`). The catalog is the standard
  set under Mattermost shortcodes, nine category tabs, search from two
  characters, skin tones, and a Recent row from this session's picks. A
  pick inserts the glyph into the composer. The hover toolbar and the
  chips' add button keep the six quick reactions in a `menu`, with "More
  emoji…" at the end opening the same picker for that post.
- **Account menu** (PLAN §3 M1 presence; §4.5 sign out) hangs off the
  title bar avatar, which carries the presence dot (`design-19`). A `menu`
  with a `label` row ("Arya · @arya"), the four presences as radio items,
  "Set a status…" (or "Status: …" once set) opening a small dialog with one
  field and Clear, Cancel, Done, then Preferences and "Sign out…". Signing
  out confirms in an `alert_dialog` with the destructive button named for
  the action, and returns to the sign-in card.
- **Composer autocomplete and typing** (PLAN §4.3; §3 M1 typing) use
  gpuicn's `Composer` triggers (`design-22`). `@` lists channel members by
  username prefix or display name (avatar, name, `@username`), `~` lists
  public and private channels with their Hash or Lock icon, and `:` lists
  emoji by shortcode from two characters with the glyph in front, eight
  rows at most. Picking inserts `@username `, `~channel ` or the glyph.
  Who is typing shows as one `meta` line between the post list and the
  composer ("Priya is typing…", "Sam and Lena are typing…", "Several
  people are typing…" beyond two), first names only, so the composer
  never jumps.
- **Composer attachments** (PLAN §4.3, §4.6 Files) sit in an
  `attachment_group` directly above the composer card (`design-23`):
  gpuicn `Attachment` thumbnails for images and cards with a type icon,
  the name and "PDF · 2.4 MB" for anything else. Files arrive from the
  Attach button or Cmd+U (the platform file dialog), from dropping onto
  the composer, which tints it `accent` while hovering, and from pasting
  an image, which becomes "Pasted image.png". A chip shows "Uploading…
  60%" with a progress bar, then its meta line; a failed one turns
  `destructive`, says "Upload failed" and gets Retry. Each has a remove
  button. Send stays disabled while any file is uploading or failed, and
  a post may be files alone; the tray clears on send.
- **Files on posts and the lightbox** (PLAN §4.6 Files, gpuicn G8) put a
  post's files under its text as the same `Attachment` chips: image
  thumbnails in one row, then cards with the type icon, name and
  "PDF · 2.4 MB" (`design-24`). Clicking a card asks sync to open the
  file; clicking a thumbnail opens gpuicn's `lightbox` over the window
  with that post's images only, "1 of 2", the file name and "Author ·
  time" as the caption, zoom, arrows and ←/→, an Open (download) action
  before Close, and Escape returning focus. The thread pane and the Saved,
  Pinned, Mentions and Search listings show the same chips and lightbox.
- **Editing in place** (PLAN §4.3 edit): Edit in a post's menu, or Up in
  an empty composer for the last own post, swaps the post body for a
  `Composer` holding the text with the caret at the end, a `meta` hint
  "Enter to save · Escape to cancel" and Ghost Cancel and Default Save
  buttons, and scrolls the row into view (`design-25`). Enter or Save
  keeps the text and adds "(edited)" at once; Escape, Cancel, an unchanged
  or emptied body closes the editor with nothing sent. Shift+Up in an
  empty composer still opens the newest post's thread. Focus returns to
  the main composer either way.
- **Channel info** (PLAN §3 M1 navigation: leave a channel, mute) opens
  from the header's info button or its members count (`design-26`). The
  pane starts with the channel name, purpose and header, then one row of
  Sm chips: Favorite/Favorited and Mute/Muted as Outline that turn
  Secondary when on, "N pinned" opening the Pinned listing, and Leave as a
  `destructive` Ghost button on its own line (not for DMs and groups).
  Leave confirms in an `alert_dialog` naming the channel. "MEMBERS · 42"
  lists the loaded profiles as 36px rows with a Sm avatar carrying the
  presence dot, the name and `@handle`; a row opens the same profile card
  as avatars in the transcript, and "Show all 42 members" asks sync for
  the rest. The header star and the pane's Favorite chip stay in step.
- **Live transcript** (PLAN §3 M1 reading: jump to unread, jump to
  latest; §4.5 unread correctness): a channel opens at its "New messages"
  line when it has one, so nothing unread scrolls past, and the list's
  own pill reads "Jump to latest" from there (`design-27`). Posts the
  server sends are appended through `ChannelView::append_posts`, which
  keeps the scroll position: at the end they simply appear, and while the
  reader is scrolled up the pill turns `primary` and counts them ("2 new
  messages"), clearing once the list is back at the end. `update_post`
  and `remove_post` apply server edits and deletes in place. In preview
  two posts land in #general twelve seconds after it opens.
- **Browse channels** (PLAN §3 M1 "Finding people and channels"): the
  sidebar's pencil is a gpuicn `Menu` with "New message", "Browse
  channels" and "Create channel" (`design-28`). Browse is a dialog: title,
  "11 public channels in Matty", a search field that filters on name and
  purpose as you type, then one row per channel with `#`, the name, the
  member count in `meta` and the purpose on a second line. Channels the
  user is already in show an `Outline` "View" that opens them; the rest
  show a `Default` "Join", which closes the dialog, adds the row to the
  Channels category and opens the channel at its empty state. Rows are
  clickable too.
- **New message and Create channel** (PLAN §3 M1): "New message" is a
  dialog with a people search over the team's profiles; each row has the
  presence avatar, name, `@handle` and position, and picking one opens
  the existing conversation or starts a new one under Direct messages
  (`design-29`). "Create channel" is a short form: a required Name whose
  description shows the address it will get ("~design-reviews", by
  Mattermost's handle rules), an optional Purpose, and a Public/Private
  radio group with one line each on what it means; Create stays disabled
  until the handle is two characters long, and the new channel opens at
  its empty state with the purpose as its line. Sync owns both on the
  server (PLAN §4.3).
- **Preferences** is a dialog, not a window: sections under `label`
  headings, joined toggle groups for theme (System, Light, Dark), text
  size (90 to 130 percent) and clock (12-hour, 24-hour), switches with a
  one-line `meta` explanation. The clock applies to every time Matty
  prints: post headers, gutter times, "Last reply", the Threads list
  (`design-18`); 24-hour is the default. A compact message density waits
  on gpuicn's `Message` growing one; Matty does not fake it with
  continuation rows.
  "Send message with" picks Enter (default; Shift+Enter for a new line) or
  Ctrl+Enter, Cmd+Enter on macOS (Enter then starts a new line); the
  modifier combination always sends, whichever is chosen, in the channel
  composer and in thread replies (`design-30`).
  Every change applies behind the dialog as it is made; the only button is
  Done.
- **Channel menu** (PLAN §3 M1 channel management): the channel name in
  the header is a gpuicn `Menu` trigger with a chevron: "Edit header…",
  "Edit purpose…", "Rename channel…" and "Leave channel…" (`design-31`);
  conversations with people only get the header. The three edits share
  one dialog (`channel_settings.rs`): a title and one line on where the
  text shows, a `Field` prefilled with the current value, a live
  "67 / 1024" count that turns into the field error past Mattermost's
  limit, Enter saves. Leave uses the same alert dialog as the info pane
  and lands on the first remaining public channel.
- **Notification preferences** per channel (PLAN §3 M1): the first entry
  of the channel menu opens a dialog with a Mute switch, "Desktop
  notifications" and "Mobile notifications" radio groups (Use my default,
  All new messages, Mentions and direct messages, Nothing), and switches
  for ignoring @channel mentions and marking unread for mentions only
  (`design-32`). Changes apply on Save as one
  `ChannelEvent::SetNotifications`, since Mattermost stores them on the
  membership in one call; the Mute switch and the info pane's Mute chip
  stay in step through `ChannelModel.muted`.
- **Add members** (PLAN §3 M1): from the channel menu or the empty
  state's button, a dialog with a people search and one checkbox row per
  person (avatar, name, `@handle`); the whole row toggles, and the primary
  button counts the choice ("Add 2 people") and stays disabled at zero
  (`design-33`). Adding bumps the header's member count at once and emits
  `ChannelEvent::AddMembers` for sync.
- **Who reacted**: hovering a reaction chip shows gpuicn's tooltip with
  the names, "You, Sam Okafor and 2 others reacted with 👍", built from
  `PostReaction::summary` (`design-30`). "You" comes first, three names at
  most, and while sync still has only the count the text falls back to
  "3 others reacted".
- **Post markers** (PLAN §4.2): a pinned or saved post says so after its
  time, "Pinned" with the pin icon and "Saved" with the bookmark, in
  Mattermost's order and before "(edited)" (`design-34`). The markers use
  the meta style in the muted color so the author stays the loudest word,
  and the same `post_markers` helper serves the channel and the thread
  pane. The icon is its own header child because the header aligns on
  baselines; a nested row sat two pixels low.
- **Dialog shortcuts** (PLAN §4.4): Cmd/Ctrl+Shift+K opens New message,
  as in Mattermost, and two Matty additions open the other sidebar-menu
  dialogs: Cmd/Ctrl+Shift+J for Browse channels and Cmd/Ctrl+Shift+N for
  Create channel (`design-35`). The shortcut sheet lists them from the
  same table, so the sheet and the bindings cannot drift apart.
- **Team rail states** (§4 above, PLAN §3 M1): the rail is its own view,
  `rail.rs`, with a `Team` per square. The current team is a primary
  button, so its hover stays primary; the others are ghost buttons on the
  sidebar accent. A team with mentions wears a count badge on its top-right
  corner and an unread team a dot, each ringed in the rail's color so it
  stands off the tile (`design-36`). Clicking a square makes it current and
  emits `RailEvent::Select`; the dashed square emits `RailEvent::Join`.
  Switching and joining teams are sync's (M2).
- **Channel row options** (PLAN §3 M1): hovering a sidebar row swaps its
  count or dot for a "…" button, gpuicn's `sidebar_menu_action`, and the
  button or a right-click opens one shared menu for that row: Mark as read
  (while unread), Add to or Remove from favorites, Mute or Unmute, then
  Leave channel or Close conversation (`design-37`). Favoriting moves the
  row between Favorites and its home category at once, and the header
  star follows; muting dims the row as before. The button stops its click
  so the row beneath does not open the channel.

- **Toasts** (PLAN §4.5): gpuicn's `ToastState` is mounted once at the
  window root, bottom-left, three deep. A finished download shows the file
  name over "Saved to ~/Downloads" for six seconds; a failure shows one
  sentence for eight (`design-38`). "Show in folder" and "Retry" wait on
  gpuicn#95 (action buttons on a notice); until then the file opens on its
  own and the error says what to do. In preview, clicking a file card shows
  the download toast.
- **Jump to a post** (PLAN §4.2): "Jump" in Saved, Pinned, mentions and
  search results opens the channel, scrolls the post into view and tints
  it with gpuicn's permalink highlight, amber with a gutter bar, for four
  seconds (`design-39`). The tint wins over the mention tint because the
  reader asked for that row. A jump to a post that is not loaded yet waits
  for the channel or page that carries it (`ChannelView::jump_to`).

- **Keyboard message selection** (PLAN §4.2): Shift+Tab from the composer
  selects the newest post and gpuicn's `MessageHighlight::Selected` marks
  it, keeping its action bar visible; Up and Down move the selection and
  Escape returns focus to the composer (`design-40`). Single keys act on
  the selected post: R replies, E edits and Delete removes your own, + opens
  the emoji picker, P pins, S saves, U marks unread from there. Keys with a
  modifier stay with the keymap, and nothing fires while no post is
  selected, so ordinary typing is never stolen.
- **Message context menu** (PLAN §4.2): right-clicking a message selects it
  and opens a gpuicn `Menu` at the pointer with Reply in thread and Add
  reaction, then the toolbar's More items (Copy text, Pin, Save) plus Mark
  as unread, and Edit and Delete on your own posts (`design-41`). The menu
  returns focus to the transcript when it closes, so the selection keys
  keep working, and Escape then hands focus back to the composer.
- **Thread pane selection** (PLAN §4.3): the thread transcript takes the
  same keyboard selection as the channel. Shift+Tab from the reply composer
  selects the newest reply, Up and Down move over the root and replies and
  skip the replies divider, and Escape returns to the composer. A
  right-click selects the row and opens a smaller menu: an Add reaction
  submenu with the quick reactions, Copy text, and Retry and Delete on a
  reply that never reached the server (`design-42`). Pin and Save stay on
  the channel row until sync can act on a thread post.
- **Older messages** (PLAN §4.2): the transcript's first row belongs to the
  channel's history. When it scrolls into view the channel asks sync for an
  older page (`ChannelEvent::NearTop`, once per page) and shows a spinner
  with "Loading older messages…"; the page arrives through
  `ChannelView::prepend_posts`, which keeps the reader's place because
  gpuicn's list anchors the scroll on the row that was at the top. At the
  oldest post the row reads "This is the start of #channel."; a failed page
  says so and offers Retry (`design-43`). Preview starts at the beginning.
- **Message display** (PLAN §4.2): Preferences gains "Message display",
  Standard or Compact, stored as `display` in settings. Compact is gpuicn's
  `MessageDensity::Compact` from 0.9.0: one line per post, the time always
  in the gutter, the name starting the body line, tighter rows. It applies
  to the channel transcript, the thread pane and the listings at once, and
  the lists remeasure when it changes (`design-44`).
- **Text selection across messages** (PLAN §4.2): the rail, sidebar,
  channel and right pane sit inside one gpuicn `selection_root`, so a drag
  selects text from one message into the next, including code blocks, and
  Cmd/Ctrl+C copies it as plain text in reading order; a focused text field
  keeps its own copy (`design-45`). "Copy as Markdown" on a single post is
  the context menu's job.
- **Copy a message** (PLAN §4.2): the context menu and the "…" menu offer
  "Copy text", the post rendered as plain text (links become their labels,
  lists keep their markers, code stays as typed), and "Copy as Markdown",
  the Mattermost source verbatim, for pasting into another editor. Both
  live in the thread pane too, and they sit before Pin and Save because
  copying is the common case (`design-46`).
- **Newer messages after a jump** (PLAN §4.2): a jump to a post the
  loaded channel does not hold asks sync for the posts around it, and the
  transcript then stops short of the newest post. Its bottom row mirrors
  the top one: showing it asks for the next newer page with "Loading newer
  messages…", a failed page offers Retry, and the row is empty once the
  transcript runs to the end, so the usual view is unchanged
  (`design-47`).
- **Composer formatting shortcuts** (PLAN §4.3): Cmd/Ctrl+B, +I and
  +Alt+K are Mattermost's bold, italic and link; Cmd/Ctrl+Shift+X and
  +Shift+C are Slack's strikethrough and inline code. Each wraps the
  selection in Markdown, the way the toolbar buttons do, and a link leaves
  "url" selected so typing replaces it. They work in the channel, thread
  and edit composers and are listed in the shortcut sheet; Cmd/Ctrl+K alone
  stays the quick switcher (`design-48`).
- **Narrow windows, right pane** (PLAN §4.1): below 900px the right pane
  no longer splits the channel; it opens as a gpuicn drawer from the right
  edge at its saved width over the channel, with the usual scrim. Escape,
  the backdrop and the pane's own close button all close it, and widening
  the window past 900px turns it back into the split with nothing lost
  (`design-49`).
- **Narrow windows, sidebar** (PLAN §4.1): below 640px the sidebar leaves
  the split and the title bar gains a panel button at the far left. It and
  Cmd/Ctrl+Shift+D open the sidebar as a drawer from the left edge at its
  saved width; picking a channel, Threads or Drafts closes it, as do
  Escape and the backdrop. The team rail stays put, so switching teams
  never needs the drawer (`design-50`).
- **Tooltips on icon-only buttons** (PLAN §5 visual direction, §8): every
  icon-only button shows a tooltip naming it, with the shortcut after two
  spaces when the keymap has one: the channel header's favorite,
  members, pinned and info, the composer toolbar ("Bold  ⌘B"), send
  ("Send  ↵", or ⌘↵ when the setting says so), the right pane's expand,
  close, attach, emoji and reply, the sidebar's pencil, and the message
  hover actions. The label is the aria label, so pointer and screen reader
  users hear the same name (`design-51`).
- **Contrast** (PLAN §8): every text token reads at 4.5:1 on each surface
  it sits on and every presence color at 3:1, in both themes, checked by
  `scripts/contrast.py`. Dark `primary` moved from `#4a6de8` (3.97:1 as a
  link) to `#6688f2` with near-black text on buttons; light `destructive`
  to `#d42020` so sign-in errors pass on the sidebar surface; light `away`
  to `#cc7a00` for the presence dot and favorite star (`design-53`).
- **Motion** (DESIGN §1, PLAN §5 and §8): the connection banner slides open
  and closed over 200ms and a sidebar category's chevron turns from right to
  down over 150ms, both through gpuicn's `transition_value`, so the theme's
  timing and the OS reduced-motion setting govern them. Preferences → Window
  gains "Reduce motion", which sets the theme's `motion.reduced` and snaps
  every gpuicn transition (drawers, switches, the sidebar) as well. Nothing
  in the message list moves (`design-54`).
- **Accessible names** (PLAN §8): every control has a name a screen reader
  can say. gpuicn's `Button::child` adds content without naming the button,
  so text buttons use `label` and icon or mixed buttons `aria_label`; a
  clickable row (a browse or New message result, a Threads row, the reply
  footer under a post) takes `Role::Button` and a name that says what it
  does ("Join #design", "Message Priya Nair", "Open thread, 3 replies").
  `scripts/check-accessible-names.py` lists any Button that slipped
  (`design-55`). Nothing changes on screen.
- **Sizes live in `style.rs`** (DESIGN §3, §5): the audit of every size
  literal outside `style.rs` moved the ones views repeated into `metrics`
  (`ICON_SMALL` 14, `ICON_TINY` 12, `ROW_SLOT` 20, `TYPING_LINE` 20,
  `EMPTY_MAX_WIDTH` 420, `THREADS_LIST_WIDTH` 360, `DRAFTS_MAX_WIDTH` 760)
  and pointed the quick switcher at `SWITCHER_MAX_WIDTH`. The right pane's
  limits in §4 now match the code (320 to 640). Pixels are identical
  (`design-56`).
- **Dark sweep** (DESIGN §2): a pass over every dialog, pane, menu and
  popover in dark mode. Dark `muted` was `#1d2027`, one step from `card`
  and `popover` (`#1c1f26`), so avatar fallbacks and icon frames vanished
  on the profile card, New message and Browse dialogs and the quick
  switcher; it is now `#262a33` (1.15:1 on `popover`, `muted-foreground`
  still 4.66:1 on it). Tooltips no longer linger over a menu or popover
  opened from their control: `style::tipped_when` drops the tooltip while
  the emoji picker or a post's action menu is open. Light mode only gains
  the tooltip fix (`design-57`).

## 5. Components

- Every control is a gpuicn component with default styling; Matty passes
  tokens and `Styled` geometry only. If a look is impossible without
  restyling, it becomes a gpuicn issue, not a local override.
- Icons are Lucide at 16px in chrome, 20px in empty states, 14px inside
  chips, badges and menu rows, 12px for chevrons and kind glyphs beside
  text (`metrics::ICON`, `ICON_LARGE`, `ICON_SMALL`, `ICON_TINY`). Stroke
  color follows the text color around it. Every size and width a view
  uses is a `metrics` constant in `style.rs`, never a literal.
- Tooltips on every icon-only button, with the shortcut when there is one.
- Empty states are built by `style::empty_state` (gpuicn `Empty` with its
  icon media sized to the 40px circle) or `style::centered_empty`; a view
  never assembles one by hand.

## 6. Screens that set the bar

Screenshots live in `docs/screenshots/`; the `design-*` files are the
reference for each pull request in this series. A change that moves away
from them needs a reason in its pull request.

To look at the design before sync exists, run the app with
`MATTY_PREVIEW=1`: the views render the fixtures in
`crates/matty-app/src/preview.rs` instead of their loading and empty
states. The `design-*` screenshots are taken this way.
