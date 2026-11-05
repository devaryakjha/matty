#!/usr/bin/env python3
"""Records matty-api decoding fixtures from the seeded development server.

    scripts/dev-server reset && scripts/seed.py && scripts/record-fixtures.py

Writes crates/matty-api/tests/fixtures/v<major>.<minor>/*.json from the
server's real responses, signed in as the seed's `tester`. It reads ids from
docker/seed-output.json, and adds a few posts to `dev/random` to record
embeds, reactions, edits, deletes and thread reads, so record against a
throwaway server only. Only the Python standard library is used.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import tempfile
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SEED = ROOT / "docker" / "seed-output.json"
PREVIEW_PARENT = "/mattermost/client/"
# Pages the server serves itself under /static/, so link previews work
# without internet access (compose allows internal connections to localhost).
PREVIEW_PAGES = {
    "page.html": """<!doctype html><html><head><title>Matty preview page</title>
<meta property="og:title" content="Matty preview page">
<meta property="og:description" content="A page with Open Graph tags for link preview fixtures.">
<meta property="og:site_name" content="Matty dev server">
<meta property="og:type" content="website">
<meta property="og:image" content="http://localhost:8065/static/images/c_avatar.png">
</head><body>preview</body></html>""",
    "plain.html": "<!doctype html><html><head><title>Plain page</title></head><body>no tags</body></html>",
}


# The server reaches an integration this script runs through this name
# (docker/compose.yml maps it to the host and lets the server call it).
INTEGRATION_HOST = "host.docker.internal"
RECORDER_DIALOG = {
    "callback_id": "recorder-dialog",
    "title": "Recorder dialog",
    "introduction_text": "Fields of **every** kind.",
    "submit_label": "Send",
    "notify_on_cancel": True,
    "state": "recorder-state",
    "elements": [
        {"display_name": "Name", "name": "name", "type": "text", "placeholder": "Your name",
         "help_text": "Type bad to fail", "min_length": 2, "max_length": 40},
        {"display_name": "Age", "name": "age", "type": "text", "subtype": "number", "optional": True},
        {"display_name": "Notes", "name": "notes", "type": "textarea", "optional": True, "default": "hello"},
        {"display_name": "Color", "name": "color", "type": "select", "default": "blue",
         "options": [{"text": "Red", "value": "red"}, {"text": "Blue", "value": "blue"}]},
        {"display_name": "Owner", "name": "owner", "type": "select", "data_source": "users", "optional": True},
        {"display_name": "Agree", "name": "agree", "type": "bool", "placeholder": "I agree", "default": "true",
         "optional": True},
        {"display_name": "Size", "name": "size", "type": "radio", "default": "s",
         "options": [{"text": "Small", "value": "s"}, {"text": "Large", "value": "l"}]},
    ],
}


class Integration:
    """A tiny integration on this machine: it answers post actions, dialog
    submissions and the `/recorder` slash command the way real ones do, so
    the server's own answers to them can be recorded."""

    def __init__(self, server: str):
        self.server = server
        self.dialogs_opened = 0
        integration = self

        class Handler(BaseHTTPRequestHandler):
            def do_POST(self):
                raw = self.rfile.read(int(self.headers.get("Content-Length") or 0)).decode()
                if "form" in self.headers.get("Content-Type", ""):
                    body = dict(urllib.parse.parse_qsl(raw))
                else:
                    body = json.loads(raw or "{}")
                self.answer(integration.handle(self.path, body))

            def answer(self, value):
                data = json.dumps(value).encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

            def log_message(self, *args):
                pass

        self.httpd = ThreadingHTTPServer(("0.0.0.0", 0), Handler)
        self.url = f"http://{INTEGRATION_HOST}:{self.httpd.server_address[1]}"
        threading.Thread(target=self.httpd.serve_forever, daemon=True).start()

    def open_dialog(self, trigger_id: str) -> None:
        body = json.dumps({"trigger_id": trigger_id, "url": f"{self.url}/dialog",
                           "dialog": RECORDER_DIALOG}).encode()
        req = urllib.request.Request(f"{self.server}/api/v4/actions/dialogs/open", data=body, method="POST",
                                     headers={"Content-Type": "application/json"})
        with urllib.request.urlopen(req, timeout=30):
            self.dialogs_opened += 1

    def action(self, id_: str, name: str, action: str, **extra) -> dict:
        return {"id": id_, "name": name, "type": "button",
                "integration": {"url": f"{self.url}/action", "context": {"action": action}}, **extra}

    def handle(self, path: str, body: dict):
        if path == "/action":
            action = (body.get("context") or {}).get("action")
            if action == "update":
                return {"update": {"message": "Approved by the recorder", "props": {"attachments": [{
                    "fallback": "Approved", "color": "good", "title": "Deploy approved",
                    "actions": [self.action("undo", "Undo", "ephemeral")]}]}},
                        "ephemeral_text": "Approved."}
            if action == "dialog":
                self.open_dialog(body["trigger_id"])
                return {}
            return {"ephemeral_text": f"You chose {body.get('context', {}).get('selected_option', action)}."}
        if path == "/dialog":
            name = (body.get("submission") or {}).get("name")
            if body.get("cancelled") or name not in ("bad", "fail"):
                return {}
            if name == "bad":
                return {"errors": {"name": "That name is taken."}}
            return {"error": "The deploy service is down."}
        if path == "/command":
            text = body.get("text", "").strip()
            if text == "dialog":
                self.open_dialog(body["trigger_id"])
                return {}
            if text == "goto":
                return {"goto_location": "/dev/channels/design"}
            if text == "shout":
                return {"response_type": "in_channel", "text": "Posted for everyone."}
            return {"response_type": "ephemeral", "text": f"You ran /recorder {text}"}
        return {}


EMAIL = re.compile(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}")
SECRET_KEYS = {"token", "password", "auth_data", "mfa_secret", "csrf"}


def sanitize(value, key: str = ""):
    """Blanks secrets and replaces addresses outside example.com, so a
    recording never holds a session token or a real email. The seed only
    makes example.com users; this guards against recording another server."""
    if isinstance(value, dict):
        return {k: sanitize(v, k) for k, v in value.items()}
    if isinstance(value, list):
        return [sanitize(v, key) for v in value]
    if isinstance(value, str):
        lower = key.lower()
        if value and (lower in SECRET_KEYS or lower.endswith("_token")):
            return ""
        return EMAIL.sub(lambda m: m.group(0) if m.group(0).endswith("@example.com")
                         else "redacted@example.com", value)
    return value


class Session:
    def __init__(self, server: str, login: str, password: str):
        self.api = server.rstrip("/") + "/api/v4"
        _, headers = self.call("POST", "users/login", {"login_id": login, "password": password},
                               auth=False, with_headers=True)
        self.token = headers["Token"]

    def call(self, method: str, path: str, body=None, *, auth=True, with_headers=False, ok_error=False):
        headers = {"Content-Type": "application/json"}
        if auth:
            headers["Authorization"] = f"Bearer {self.token}"
        data = json.dumps(body).encode() if body is not None else None
        req = urllib.request.Request(f"{self.api}/{path}", data=data, method=method, headers=headers)
        try:
            with urllib.request.urlopen(req, timeout=60) as resp:
                value = json.loads(resp.read() or b"null")
                return (value, resp.headers) if with_headers else value
        except urllib.error.HTTPError as err:
            if ok_error:
                return json.loads(err.read())
            raise SystemExit(f"{method} {path}: {err.code} {err.read().decode(errors='replace')}") from None


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--version", help="fixture directory label, such as 10.5 (default: the server's)")
    parser.add_argument("--only", nargs="+", metavar="NAME",
                        help="write only these fixtures, leaving the others as they are")
    args = parser.parse_args()

    seed = json.loads(SEED.read_text())
    server = seed["server"]
    tester = Session(server, seed["tester"]["username"], seed["password"])
    admin = Session(server, seed["admin"]["username"], seed["password"])
    me = seed["tester"]["id"]
    team = next(t["id"] for t in seed["teams"] if t["name"] == "dev")
    channels = {(c["team"], c["name"]): c for c in seed["channels"]}
    busy = channels[("dev", "busy")]
    design = channels[("dev", "design")]["id"]
    random_ch = channels[("dev", "random")]["id"]

    config = tester.call("GET", "config/client?format=old", auth=False)
    version = args.version or ".".join(config["Version"].split(".")[:2])
    out = ROOT / "crates" / "matty-api" / "tests" / "fixtures" / f"v{version}"
    out.mkdir(parents=True, exist_ok=True)

    def save(name: str, value) -> None:
        if args.only and name not in args.only:
            return
        (out / f"{name}.json").write_text(json.dumps(sanitize(value), indent=2, ensure_ascii=False) + "\n")
        print(f"recorded {name}")

    def get(name: str, path: str, session=tester) -> object:
        value = session.call("GET", path)
        save(name, value)
        return value

    # Connect and initial load.
    get("ping", "system/ping")
    save("config_client", config)
    get("license_client", "license/client?format=old")
    get("users_me", "users/me")
    get("my_teams", "users/me/teams")
    get("my_team_members", "users/me/teams/members")
    get("team_unreads", f"users/{me}/teams/unread")
    get("my_channels", f"users/me/teams/{team}/channels")
    get("my_channel_members", f"users/{me}/teams/{team}/channels/members")
    get("sidebar_categories", f"users/me/teams/{team}/channels/categories")
    get("preferences", "users/me/preferences")
    get("emoji_page", "emoji?page=0&per_page=200")
    get("users_page", "users?page=0&per_page=200")
    get("channel_stats", f"channels/{design}/stats")
    save("users_by_ids", tester.call("POST", "users/ids", [me]))
    save("statuses", tester.call("POST", "users/status/ids", [me]))
    save("login_invalid_credentials", tester.call(
        "POST", "users/login", {"login_id": seed["tester"]["username"], "password": "wrong"},
        auth=False, ok_error=True))

    # Reading posts. The tester has not viewed busy, so it is unread from the start.
    get("posts_around_unread", f"users/{me}/channels/{busy['id']}/posts/unread?limit_before=10&limit_after=10")
    get("channel_posts", f"channels/{busy['id']}/posts?page=0&per_page=30")
    get("channel_posts_crt", f"channels/{busy['id']}/posts?page=0&per_page=30&collapsedThreads=true")
    root = busy["thread_roots"][0]
    get("post_thread", f"posts/{root}/thread?perPage=10&direction=down&fromCreateAt=0")
    image_post = next(c["image_posts"][0] for c in seed["channels"] if c["image_posts"])
    get("post_with_files", f"posts/{image_post}")

    # Embeds, from pages the server itself serves.
    # Copied in rather than written with a shell: newer images have no `sh`.
    with tempfile.TemporaryDirectory() as pages:
        site = Path(pages) / "matty-preview"
        site.mkdir()
        for name, html in PREVIEW_PAGES.items():
            (site / name).write_text(html)
        subprocess.run([str(ROOT / "scripts" / "dev-server"), "cp", str(site), PREVIEW_PARENT], check=True)
    static = f"{server}/static"
    embeds = {
        "post_opengraph": f"Preview {static}/matty-preview/page.html",
        "post_image_link": f"{static}/images/appIcons.png",
        "post_link": f"Plain {static}/matty-preview/plain.html",
    }
    for name, message in embeds.items():
        post = tester.call("POST", "posts", {"channel_id": random_ch, "message": message})
        get(name, f"posts/{post['id']}")
    post = tester.call("POST", "posts", {"channel_id": random_ch, "message": "", "props": {"attachments": [{
        "fallback": "Build 42 passed", "color": "#2eb67d", "pretext": "CI finished",
        "author_name": "ci-bot", "title": "Build 42", "title_link": "https://example.com/builds/42",
        "text": "All **312** tests passed.", "footer": "ci",
        "fields": [{"title": "Branch", "value": "master", "short": True},
                   {"title": "Duration", "value": 95, "short": True}],
    }]}})
    get("post_attachment", f"posts/{post['id']}")
    emoji = seed["emoji"][0]["name"]
    post = tester.call("POST", "posts", {"channel_id": random_ch, "message": f"Custom :{emoji}: emoji"})
    for session, user, name in [(tester, me, emoji), (admin, seed["admin"]["id"], "+1")]:
        session.call("POST", "reactions", {"user_id": user, "post_id": post["id"], "emoji_name": name})
    get("post_reactions", f"posts/{post['id']}")

    # Writing posts.
    pending = f"{me}:{int(time.time() * 1000)}"
    created = tester.call("POST", "posts", {"channel_id": random_ch, "message": "Created by the recorder",
                                            "pending_post_id": pending})
    save("post_created", created)
    since = created["create_at"]
    save("post_patched", tester.call("PUT", f"posts/{created['id']}/patch", {"message": "Edited by the recorder"}))
    doomed = tester.call("POST", "posts", {"channel_id": random_ch, "message": "Deleted by the recorder"})
    save("status_ok", tester.call("DELETE", f"posts/{doomed['id']}"))
    get("channel_posts_since", f"channels/{random_ch}/posts?since={since}")

    # Threads: replying follows the thread; the admin's reply makes it unread.
    root_post = tester.call("POST", "posts", {"channel_id": random_ch, "message": "A thread to follow"})
    tester.call("POST", "posts", {"channel_id": random_ch, "root_id": root_post["id"], "message": "My reply"})
    admin.call("POST", "posts", {"channel_id": random_ch, "root_id": root_post["id"],
                                 "message": f"@{seed['tester']['username']} a reply to read"})
    get("user_threads", f"users/{me}/teams/{team}/threads?extended=true")
    get("user_threads_unread", f"users/{me}/teams/{team}/threads?unread=true")
    save("thread_read", tester.call(
        "PUT", f"users/{me}/teams/{team}/threads/{root_post['id']}/read/{int(time.time() * 1000)}", {}))
    tester.call("DELETE", f"users/{me}/teams/{team}/threads/{root_post['id']}/following")
    tester.call("PUT", f"users/{me}/teams/{team}/threads/{root_post['id']}/following", {})

    # Read state.
    save("channel_viewed", tester.call("POST", "channels/members/me/view", {
        "channel_id": busy["id"], "prev_channel_id": random_ch, "collapsed_threads_supported": True}))
    # M1 features: search, saved and pinned posts, browsing, DMs, autocomplete.
    users = {u["username"]: u["id"] for u in seed["users"]}
    findable = tester.call("POST", "posts", {"channel_id": random_ch,
                                             "message": "The recorder hides a zebrafish here"})
    save("search_posts", tester.call("POST", f"teams/{team}/posts/search", {
        "terms": "zebrafish", "is_or_search": False, "time_zone_offset": 0, "page": 0, "per_page": 20,
        "include_deleted_channels": False}))
    get("saved_posts", f"users/{me}/posts/flagged?page=0&per_page=20")
    get("pinned_posts", f"channels/{busy['id']}/pinned")
    tester.call("POST", f"posts/{findable['id']}/pin", {})
    tester.call("POST", f"posts/{findable['id']}/unpin", {})
    get("reactions", f"posts/{post['id']}/reactions")
    get("public_channels", f"teams/{team}/channels?page=0&per_page=60")
    save("search_channels", tester.call("POST", f"teams/{team}/channels/search", {"term": "des"}))
    get("autocomplete_channels", f"teams/{team}/channels/autocomplete?name=eng")
    empty = channels[("dev", "empty")]["id"]
    tester.call("DELETE", f"channels/{empty}/members/{me}")
    save("channel_member_added", tester.call("POST", f"channels/{empty}/members", {"user_id": me}))
    save("direct_channel", tester.call("POST", "channels/direct", [me, users["ada.lovelace"]]))
    save("group_channel", tester.call("POST", "channels/group",
                                      [me, users["ada.lovelace"], users["alan.allen"]]))
    get("users_autocomplete", f"users/autocomplete?name=a&in_team={team}")
    get("users_autocomplete_in_channel", f"users/autocomplete?name=a&in_team={team}&in_channel={design}")

    record_integrations(save, server, tester, admin, team, random_ch)

    save("channel_unread_at", tester.call("POST", f"users/{me}/posts/{busy['pinned_posts'][0]}/set_unread",
                                          {"collapsed_threads_supported": True}))


def record_integrations(save, server: str, tester: Session, admin: Session, team: str, channel: str) -> None:
    """Interactive messages, dialogs and slash commands (#210, #211),
    answered by an integration this script runs."""
    integration = Integration(server)
    hook = integration.url
    # Interactive message: buttons and menus the server hands to the
    # integration; it strips their URLs and adds cookies when serving them.
    menu = {"type": "select", "integration": {"url": f"{hook}/action", "context": {"action": "select"}}}
    post = tester.call("POST", "posts", {"channel_id": channel, "message": "", "props": {"attachments": [{
        "fallback": "Deploy matty to production?", "color": "warning", "pretext": "A deploy is waiting.",
        "title": "matty → production", "text": "Requested by the recorder.",
        "fields": [{"title": "Commits", "value": 14, "short": True}],
        "actions": [
            integration.action("approve", "Approve", "update", style="primary"),
            integration.action("details", "Details", "dialog"),
            integration.action("reject", "Reject", "ephemeral", style="danger", disabled=True),
            {**menu, "id": "region", "name": "Pick a region", "default_option": "eu",
             "options": [{"text": "Europe", "value": "eu"}, {"text": "US East", "value": "us"}]},
            {**menu, "id": "reviewer", "name": "Ask a reviewer", "data_source": "users"},
        ]}]}})
    post_id = post["id"]
    save("post_interactive", tester.call("GET", f"posts/{post_id}"))
    tester.call("POST", f"posts/{post_id}/actions/region", {"selected_option": "us"})
    tester.call("POST", f"posts/{post_id}/actions/details", {})
    save("post_action", tester.call("POST", f"posts/{post_id}/actions/approve", {}))
    save("post_interactive_updated", tester.call("GET", f"posts/{post_id}"))

    def submit(name: str, cancelled: bool = False):
        return tester.call("POST", "actions/dialogs/submit", {
            "type": "dialog_submission", "url": f"{hook}/dialog", "callback_id": "recorder-dialog",
            "state": "recorder-state", "channel_id": channel, "team_id": team, "cancelled": cancelled,
            "submission": {} if cancelled else {"name": name, "agree": True, "age": 3}}, ok_error=True)
    save("dialog_submit_errors", submit("bad"))
    save("dialog_submit_error", submit("fail"))
    submit("Ada")
    submit("", cancelled=True)

    # Slash commands: the server's own and an integration's.
    if not any(c["trigger"] == "recorder" for c in admin.call("GET", f"commands?team_id={team}&custom_only=true")):
        admin.call("POST", "commands", {
            "team_id": team, "trigger": "recorder", "method": "P", "url": f"{hook}/command",
            "display_name": "Recorder", "description": "Answers the fixture recorder", "username": "recorder",
            "auto_complete": True, "auto_complete_desc": "Answers the fixture recorder",
            "auto_complete_hint": "[dialog|goto|shout|text]"})
    else:
        command = next(c for c in admin.call("GET", f"commands?team_id={team}&custom_only=true")
                       if c["trigger"] == "recorder")
        admin.call("PUT", f"commands/{command['id']}", {**command, "url": f"{hook}/command"})
    get = lambda path: tester.call("GET", path)  # noqa: E731
    save("commands_autocomplete", get(f"teams/{team}/commands/autocomplete"))
    query = urllib.parse.urlencode({"user_input": "/re", "channel_id": channel})
    save("command_suggestions", get(f"teams/{team}/commands/autocomplete_suggestions?{query}"))

    def run(line: str):
        return tester.call("POST", "commands/execute", {"command": line, "channel_id": channel, "team_id": team},
                           ok_error=True)
    save("command_response_ephemeral", run("/recorder hello"))
    save("command_response_in_channel", run("/recorder shout"))
    save("command_response_goto", run("/recorder goto"))
    save("command_response_dialog", run("/recorder dialog"))
    save("command_response_builtin", run("/away"))
    run("/online")
    save("command_not_found", run("/nope"))
    integration.httpd.shutdown()
    if integration.dialogs_opened != 2:
        raise SystemExit(f"the integration opened {integration.dialogs_opened} dialogs, not 2")


if __name__ == "__main__":
    main()
