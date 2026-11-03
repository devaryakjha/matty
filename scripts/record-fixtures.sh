#!/usr/bin/env bash
# Records matty-api decoding fixtures from a running Mattermost server.
#
#   scripts/record-fixtures.sh http://localhost:8065 arya 'Password1!' 10.5
#
# Writes crates/matty-api/tests/fixtures/v<version>/*.json. Use a throwaway
# development server: the responses contain its ids, names and emails.
set -euo pipefail

server="${1:?server URL}"
login="${2:?login id}"
password="${3:?password}"
version="${4:?version label, such as 10.5}"
api="$server/api/v4"
out="$(dirname "$0")/../crates/matty-api/tests/fixtures/v$version"
mkdir -p "$out"

token=$(curl -sf -i -X POST "$api/users/login" \
    -d "{\"login_id\":\"$login\",\"password\":\"$password\"}" |
    awk 'tolower($1) == "token:" { print $2 }' | tr -d '\r')
auth=(-H "Authorization: Bearer $token")
json() { python3 -c 'import json,sys; print(json.load(sys.stdin)'"$1"')'; }

get() { # name path
    curl -sf "${auth[@]}" "$api/$2" | python3 -m json.tool --indent 2 > "$out/$1.json"
    echo "recorded $1"
}
post() { # name path body
    curl -sf "${auth[@]}" -X POST "$api/$2" -d "$3" | python3 -m json.tool --indent 2 > "$out/$1.json"
    echo "recorded $1"
}

me=$(curl -sf "${auth[@]}" "$api/users/me" | json "['id']")
team=$(curl -sf "${auth[@]}" "$api/users/me/teams" | json "[0]['id']")
channel=$(curl -sf "${auth[@]}" "$api/users/me/teams/$team/channels" |
    python3 -c 'import json,sys; print(next(c["id"] for c in json.load(sys.stdin) if c["type"] == "O"))')

get ping "system/ping"
get config_client "config/client?format=old"
get license_client "license/client?format=old"
get users_me "users/me"
get my_teams "users/me/teams"
get my_team_members "users/me/teams/members"
get team_unreads "users/$me/teams/unread"
get my_channels "users/me/teams/$team/channels"
get my_channel_members "users/$me/teams/$team/channels/members"
get sidebar_categories "users/me/teams/$team/channels/categories"
get preferences "users/me/preferences"
get emoji_page "emoji?page=0&per_page=200"
get users_page "users?page=0&per_page=200"
get channel_stats "channels/$channel/stats"
post users_by_ids "users/ids" "[\"$me\"]"
post statuses "users/status/ids" "[\"$me\"]"
curl -s -X POST "$api/users/login" -d "{\"login_id\":\"$login\",\"password\":\"wrong\"}" |
    python3 -m json.tool --indent 2 > "$out/login_invalid_credentials.json"
echo "recorded login_invalid_credentials"
