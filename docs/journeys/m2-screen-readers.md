# M2 screen reader journeys (#217)

These are the checks a person runs by hand with each platform's screen
reader on the surfaces M2 added: pop-out windows, split view, the dialogs
and panes, the bookmarks bar, message priority and acknowledgements, and
custom statuses. The M1 VoiceOver journey (`m1-alpha.md` §4) still covers
the sidebar, the message list and the composer.

## What CI already checks

`crates/matty-app/tests/accessible_surfaces.rs` opens each surface below
over the preview fixtures and fails if any interactive element in any open
window has no accessible name. A name is what a screen reader should say,
so the **Expect** column quotes those names. CI can't tell what a screen
reader actually announces, or whether focus lands in the right place. That
is what these runs record.

## How to run

- **Build:** the signed build for macOS and Windows, or the AppImage on
  Linux. Use a release build, not a dev build.
- **Screen readers:**
  - VoiceOver on macOS 13 or later.
  - NVDA (the current release) and Narrator on Windows 11.
  - Orca on GNOME under Wayland and X11.
- **Server:** use the current Mattermost release seeded with
  `scripts/dev-server`, signed in as `tester`. For the acknowledgement
  steps, a second account in the webapp sends an urgent post that
  requests acknowledgement.
- **Evidence:** record each step's announcement word for word, plus a screen
  recording, under `docs/journeys/evidence/m2-screen-readers/<platform>/`.
  A step with no announcement is a Fail ("silent gap").
- **Failures:** file an issue and link it in Notes. If the cause is in GPUI
  or gpuicn, file it there too, and add it to the upstream blockers list
  below.
- **Copying:** for each platform, copy the table into a new section under
  Runs, headed with the platform, its version and the build.

Columns: **Step**, **Do**, **Expect**, **Heard**, **Date**, **Tester**,
**Result**, **Notes**.

## Steps

| Step | Do | Expect | Heard | Date | Tester | Result | Notes |
|---|---|---|---|---|---|---|---|
| S.1 | Move to the bookmarks bar in #general | Each bookmark reads its name and kind ("Team handbook, link", "Matty spec.pdf, file"), then "Add a bookmark" and "Bookmark actions" | | | | | |
| S.2 | Open "Bookmark actions" | A menu is announced, and each item reads its name, starting with "Add a bookmark…" | | | | | |
| S.3 | Move to an urgent post that requests acknowledgement | The post reads its priority ("Urgent") with its text. The bar reads "Acknowledge", and then who already acknowledged | | | | | |
| S.4 | Press "Acknowledge", then press it again | Each press is announced: "Acknowledged" first, then "Remove acknowledgement" while it can still be undone | | | | | |
| S.5 | In the composer, open "Message priority" | A menu opens with the levels, plus "Request acknowledgement" and "Send persistent notifications" as toggles that read their state | | | | | |
| S.6 | Pick Urgent and request acknowledgement | The composer reads "Urgent · Acknowledgement requested", and its remove button reads "Remove Urgent · Acknowledgement requested" | | | | | |
| S.7 | Open a post's "More actions" | A menu is announced, and each item reads its name ("Copy link", "Pin to channel") | | | | | |
| S.8 | Open in split view (Cmd-\\ on macOS, Ctrl-\\ elsewhere) | Each pane reads its side, channel and focus ("Right pane, general, focused"), and so do "Swap panes", "Close pane" and "Resize split view" | | | | | |
| S.9 | Press F6 between the panes | Focus moves, and the newly focused pane is announced | | | | | |
| S.10 | Pop the channel out into a new window (Cmd-Shift-O on macOS, Ctrl-Shift-O elsewhere) | The new window is announced with its channel name, and the composer reads "Message" | | | | | |
| S.11 | Close the pop-out | Focus returns to the main window and is announced | | | | | |
| S.12 | Open each of these, then press Escape: the quick switcher, Browse channels, Create channel, Direct messages, Preferences, Keyboard shortcuts | On opening, the dialog's title is read and focus is in its first field ("Search commands", "Search channels", "Name (required)", "Search people"). On Escape, focus returns where it was | | | | | |
| S.13 | Open Search, Channel info, Recent mentions and Saved messages | The right pane's title is read, followed by "Expand" and "Close". Each result's "Jump" reads in turn | | | | | |
| S.14 | Set a custom status from the account menu | The dialog reads "Emoji", the status field, the Recent and Suggestions entries and the "Clear after" choice. Picking "Done" announces the change | | | | | |
| S.15 | Open a direct message with someone who has a status | The header reads their status and local time ("In a design review, 14:05 local time") | | | | | |

## Upstream blockers

Add each GPUI or gpuicn issue that leaves a step silent or wrong, with its
link and the steps it affects.

## Runs

Add one section per platform run below this line.
