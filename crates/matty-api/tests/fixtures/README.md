# API fixtures

`config_client_*.json` and `license_client_*.json` are written by hand from
Mattermost's `format=old` client config and license (every value a string)
and cover the keys `ServerCapabilities` reads. #19 replaces them with
responses recorded from each supported server version.
