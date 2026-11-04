# WebSocket event fixtures

One file per event name, each a whole frame as the server sent it.

Recorded from the seeded Mattermost 10.5 dev server by
`tests/record_ws.rs` (see the command at the top of that file).

Hand-written, because this server does not send them (they need a license,
or Mattermost 10.5 sends a different event), and marked with a `_source`
key: `channel_viewed` (10.5 sends `multiple_channels_viewed`),
`channel_scheme_updated` and `license_changed` (license),
`open_dialog` (needs an integration), `direct_added` and `group_added` (not delivered to a connection opened
before the channel existed), `draft_updated` (10.5 sends `draft_created`
for every save), `post_acknowledgement_added` and
`post_acknowledgement_removed` (license), and `preference_changed` (the
single-preference form older servers send). Their shapes follow the server
source and the recorded neighbours; replace them with recordings when a
licensed or older server is available.
