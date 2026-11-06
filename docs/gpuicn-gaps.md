# gpuicn gaps reported by Matty

Every gap Matty found in gpuicn goes to the gpuicn repository as an issue
(AGENTS.md: reusable UI comes from gpuicn, never a local substitute). This
is the list as of gpuicn `=0.9.6` (see `PINS.md`), for #196 and #197. Each
fix reaches Matty through a pin bump, since Matty uses gpuicn as a
crates.io crate with no patches or git dependency.

| Issue | Gap | Status |
|---|---|---|
| [#83](https://github.com/karyalabs/gpuicn/issues/83) | MessageList scrollbar thumb tracks measured rows, not position (Matty S1) | Fixed |
| [#88](https://github.com/karyalabs/gpuicn/issues/88) | Message: a compact density (one line per post, time in the gutter, author inline) | Fixed |
| [#94](https://github.com/karyalabs/gpuicn/issues/94) | Message: full date tooltip on the timestamp | Fixed |
| [#95](https://github.com/karyalabs/gpuicn/issues/95) | Toast: action buttons on a Notice | Fixed |
| [#97](https://github.com/karyalabs/gpuicn/issues/97) | MessageList: on_jump hook for the "Jump to latest" pill | Fixed |
| [#98](https://github.com/karyalabs/gpuicn/issues/98) | Message: on_gutter_click to select a row from its gutter | Fixed |
| [#99](https://github.com/karyalabs/gpuicn/issues/99) | Attachment: image thumbnails sized from the file's dimensions | Fixed |
| [#100](https://github.com/karyalabs/gpuicn/issues/100) | Sidebar: one tab stop per section with arrow-key (roving) focus | Fixed |
| [#101](https://github.com/karyalabs/gpuicn/issues/101) | ComposerState: change the growth cap after construction | Fixed |
| [#108](https://github.com/karyalabs/gpuicn/issues/108) | Link preview and message attachment cards for chat messages | Fixed |
| [#109](https://github.com/karyalabs/gpuicn/issues/109) | modal_viewport does not scroll when the dialog uses occlude() | Fixed |
| [#111](https://github.com/karyalabs/gpuicn/issues/111) | Click-to-load image placeholder for LinkPreview and MessageEmbed | Fixed |
| [#115](https://github.com/karyalabs/gpuicn/issues/115) | Avatar: presence/status indicator slot | Fixed |
| [#117](https://github.com/karyalabs/gpuicn/issues/117) | Menu: size to the space in the window, and open submenus on hover | Fixed |
| [#118](https://github.com/karyalabs/gpuicn/issues/118) | Popover: hover-to-open mode that keeps click and keyboard (profile hover cards) | Fixed |
| [#119](https://github.com/karyalabs/gpuicn/issues/119) | Embed: quoted-post card for permalinks and forwards | Fixed |
| [#120](https://github.com/karyalabs/gpuicn/issues/120) | Make components observable so apps can audit accessible names | Fixed |
| [#121](https://github.com/karyalabs/gpuicn/issues/121) | Sortable list: drag rows to reorder them and move them between groups | Fixed |
| [#122](https://github.com/karyalabs/gpuicn/issues/122) | ToastState: tell the caller when a toast is dismissed, or whether it is still showing | Fixed |
| [#123](https://github.com/karyalabs/gpuicn/issues/123) | Message: let the author name open something (on_author_click or an author element) | Fixed |
| [#127](https://github.com/karyalabs/gpuicn/issues/127) | Avatar status dot: size it to the rendered avatar, or allow a custom dot size | Fixed |
| [#128](https://github.com/karyalabs/gpuicn/issues/128) | Text selection: per-message copy prefix for multi-message copy | Fixed |
| [#129](https://github.com/karyalabs/gpuicn/issues/129) | Composer: no autocomplete trigger inside code spans or fences | Fixed |
| [#131](https://github.com/karyalabs/gpuicn/issues/131) | MarkdownImages: placeholder for not-yet-loaded images and known image sizes | Fixed |
| [#132](https://github.com/karyalabs/gpuicn/issues/132) | Theme tokens for Mattermost themes: link, mention highlight, separator, sidebar badges, code theme; inline Alert | Fixed |
| [#133](https://github.com/karyalabs/gpuicn/issues/133) | CommandItem: accessible description for secondary text | Fixed |
| [#134](https://github.com/karyalabs/gpuicn/issues/134) | MessageEmbed actions slot and Button custom colour | Fixed |
| [#135](https://github.com/karyalabs/gpuicn/issues/135) | Form gaps: Field for checkbox/radio, inline Alert, Textarea focus fill | Fixed |
| [#136](https://github.com/karyalabs/gpuicn/issues/136) | Composer autocomplete: let the app open suggestions for its own text range | Fixed |
| [#137](https://github.com/karyalabs/gpuicn/issues/137) | Sortable: horizontal (row) orientation | Fixed |
| [#138](https://github.com/karyalabs/gpuicn/issues/138) | DateTimePicker and AvatarGroup components | Fixed |
| [#139](https://github.com/karyalabs/gpuicn/issues/139) | Composer/Textarea: spelling underlines and a suggestions menu for app-supplied ranges | Fixed |
| [#140](https://github.com/karyalabs/gpuicn/issues/140) | Calendar and DatePicker components | Fixed |
| [#141](https://github.com/karyalabs/gpuicn/issues/141) | SelectState / Combobox: replace items in place (set_items) | Fixed |
| [#143](https://github.com/karyalabs/gpuicn/issues/143) | Localizable strings and right-to-left layout | Open: deferred to Matty #225 (localization and RTL) |

Add a row when filing a new gpuicn issue, and update its status when the
release that fixes it is pinned.
