# Matty's UI strings

Matty looks up its UI text by id with `t!` (#225). The strings live in
[Fluent](https://projectfluent.org) files, one per area of the app, under
`locales/<tag>/`: for example `locales/en/login.ftl` for the sign-in card.
English (`en`) is the source. The build joins a language's files into one
resource, so a new area only adds a file, and an id must be unique across
all of them.

A translation lives beside English as `locales/<tag>/*.ftl` and may leave
ids out: English stands in for anything missing.
