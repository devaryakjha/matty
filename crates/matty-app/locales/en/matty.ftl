# Matty's UI strings in English, the source language (#225).
#
# Ids are grouped by area: one area per section, named after the file that
# shows them. Translations live beside this file as `locales/<tag>/matty.ftl`
# and may leave ids out: English stands in for anything missing.

## Custom emoji dialog (emoji_manager.rs)

emoji-manager-title = Custom emoji
emoji-manager-description = Add an image everyone on this server can use as an emoji.
emoji-manager-choose = Choose image…
emoji-manager-choose-another = Choose another image…
emoji-manager-choose-prompt = Choose image
emoji-manager-image-hint = PNG, JPEG or GIF, up to 512 KB.
emoji-manager-image-type = Choose a PNG, JPEG or GIF image.
emoji-manager-image-size = Choose an image of 512 KB or less.
emoji-manager-image-unreadable = Couldn't read that file.
emoji-manager-name = Name
emoji-manager-name-hint = Others use it as :{ $name }: in messages and reactions.
emoji-manager-name-empty = Others use it as :name: in messages and reactions.
emoji-manager-name-long = Use { $max } characters or fewer.
emoji-manager-name-characters = Use lowercase letters, numbers, -, + and _.
emoji-manager-name-taken = There's already an emoji called :{ $name }:.
emoji-manager-add = Add emoji
emoji-manager-listed = On this server ({ $count })
emoji-manager-delete = Delete
emoji-manager-delete-confirm = Delete for everyone
emoji-manager-delete-label = Delete :{ $name }:
emoji-manager-done = Done

## Sign-in card (login.rs)

login-ldap-username = AD/LDAP username
login-email-or-username = Email or username
login-mfa-wrong = That code is not right. Enter the current code from your authenticator app.
login-mfa-needed = This account needs a multi-factor code. Sign in again.
login-address-empty = Enter your server's address, like chat.example.com.
login-address-invalid = That does not look like a server address.
login-address-scheme = The address has to start with https://.
login-password-missing = Enter your email or username and password.
login-code-short = Enter the { $digits }-digit code from your authenticator app.
login-token-missing = Paste a personal access token.
login-add-title = Add a Mattermost server
login-connect-title = Connect to your Mattermost server
login-address-hint = Enter the address you open in the browser. Matty assumes https.
login-address = Server address
login-continue = Continue
login-connect-to = Connect to { $server }
login-recent = RECENT SERVERS
login-change-server = Change server
login-change = Change
login-sign-in-title = Sign in
login-sign-in-hint = Use the account you have on this server.
login-password = Password
login-sign-in = Sign in
login-continue-with = Continue with { $provider }
login-token-show = Use a personal access token
login-token-hide-toggle = Hide personal access token
login-token-show-toggle = Use a personal access token instead
login-token = Personal access token
login-token-hint = From Profile › Security › Personal access tokens.
login-token-sign-in = Sign in with token
login-method = Sign in with
login-method-ldap = AD/LDAP
login-mfa-title = Enter your code
login-mfa-hint = This account uses multi-factor authentication. Open your authenticator app and enter the code it shows.
login-mfa-code-for = { $digits }-digit code for { $server }
login-mfa-code = Authentication code
login-verify = Verify
login-back-to-sign-in = Back to sign in
login-back = Back
login-signing-in = Signing in…
login-waiting-for = Waiting for { $provider }…
login-browser-cancel = Cancel browser sign-in
login-cancel = Cancel
login-retry-label = Try { $provider } sign-in again
login-retry = Retry
login-browser-failed-title = Browser sign-in did not finish
login-browser-failed-hint = Try again to open a new sign-in page, or cancel to pick another way in.
login-browser-waiting-title = Waiting for your browser…
login-browser-waiting-hint = Finish signing in in the browser window Matty opened. Matty continues on its own when you are done.
login-footer = Matty is a native Mattermost client. Your password goes only to your server.
