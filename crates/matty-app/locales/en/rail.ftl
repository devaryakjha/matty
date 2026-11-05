## The server and team rail (rail.rs)

# A team square's accessible name and what is unread in it.
rail-team-unread = { $team }, unread messages
rail-team-mentions = { $team }, { $count ->
        [one] { $count } mention
       *[other] { $count } mentions
    }
rail-servers = Servers
rail-server-options = Server options
rail-remove-server-item = Remove server…
rail-remove-server = Remove server
rail-remove-title = Remove { $server }?
rail-remove-title-unknown = Remove this server?
rail-remove-description = This signs you out of the server and removes Matty's copy of its messages, files and drafts from this computer.
rail-cancel = Cancel
rail-remove = Remove
rail-add-server = Add a server
rail-join-team = Join another team
