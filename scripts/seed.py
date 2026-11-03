#!/usr/bin/env python3
"""Seeds the development Mattermost server (scripts/dev-server) with test data.

    scripts/seed.py                       # default size, http://localhost:8065
    scripts/seed.py --posts 10000 --users 50
    scripts/seed.py --server http://localhost:8066

Creates an admin, users with avatars, two teams with public and private
channels, direct and group messages, custom sidebar categories, posts that
cover every Markdown construct Matty renders (PLAN 6.8), threads, reactions,
custom emoji, uploaded images, and pinned and saved posts. Credentials and ids
go to docker/seed-output.json for tests to read.

Running it again does not duplicate anything: users, teams, channels, emoji
and categories are looked up before they are created, and a channel that
already holds seeded posts is skipped. Use `scripts/dev-server reset` for a
fresh start, for example after changing --posts.

Only the Python standard library is used.
"""

from __future__ import annotations

import argparse
import concurrent.futures
import json
import random
import struct
import subprocess
import sys
import time
import urllib.error
import urllib.request
import uuid
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUTPUT = ROOT / "docker" / "seed-output.json"
PASSWORD = "Password1!"
ADMIN = "admin"
# The account tests sign in as: email/password, no MFA, no LDAP or SSO.
TESTER = "tester"
SEED_PROP = "matty_seed"

FIRST = ["ada", "alan", "barbara", "brian", "claude", "dennis", "edsger",
         "frances", "grace", "guido", "hedy", "ivan", "jean", "ken", "linus",
         "margaret", "niklaus", "radia", "rob", "sophie", "tim", "ursula",
         "vint", "whitfield", "yukihiro", "zuse"]
LAST = ["lovelace", "turing", "liskov", "kernighan", "shannon", "ritchie",
        "dijkstra", "allen", "hopper", "rossum", "lamarr", "sutherland",
        "sammet", "thompson", "torvalds", "hamilton", "wirth", "perlman",
        "pike", "wilson", "berners-lee", "franklin", "cerf", "diffie",
        "matsumoto", "konrad"]

# Ten public channels per team besides Town Square and Off-Topic.
PUBLIC_CHANNELS = ["design", "engineering", "random", "announcements",
                   "releases", "music", "busy", "markdown", "empty",
                   "long-names-are-allowed-in-mattermost-channels-too"]
PRIVATE_CHANNELS = ["leads", "security", "hiring"]
# Share of the posts each channel gets; the rest are spread evenly.
WEIGHTS = {"busy": 0.4, "markdown": 0.05, "empty": 0.0}

EMOJI = ["matty", "shipit", "partyparrot", "this-is-fine", "lgtm"]

# One sample per construct in PLAN 6.8, plus layout edge cases.
MARKDOWN = [
    "# Heading 1\n## Heading 2\n### Heading 3\n#### Heading 4\n##### Heading 5\n###### Heading 6",
    "Plain **bold**, *italic*, ***both***, ~~strikethrough~~ and `inline code`.",
    "```rust\nfn main() {\n    println!(\"hello, matty\");\n}\n```",
    "```\nno language\n    indented inside\n```",
    "    indented code block\n    second line",
    "> A block quote\n> spanning lines\n>\n> > and a nested one",
    "- one\n- two\n  - nested\n    - deeper\n- three",
    "1. first\n2. second\n   1. nested\n3. third",
    "- [ ] open task\n- [x] done task",
    "| Left | Center | Right |\n| :--- | :---: | ---: |\n| a | b | c |\n| longer cell | `code` | **bold** |",
    "[A link](https://mattermost.com) and an autolink https://github.com/devaryakjha/matty and www.example.com",
    "Hey @{user}, can you look at ~{channel}? Also @here and @channel.",
    "Emoji :tada: :+1: :white_check_mark: :{emoji}: inline",
    ":rocket: :fire: :100:",
    "Tagging #release-notes and #m0 for later",
    "```latex\n\\int_0^\\infty e^{-x^2} dx = \\frac{\\sqrt{\\pi}}{2}\n```",
    "Inline math $E = mc^2$ in a sentence.",
    "![an inline image](https://picsum.photos/id/10/400/300)",
    "Above the rule\n\n---\n\nBelow the rule",
    "Line one\nLine two after a soft break\n\nNew paragraph",
    "Escaped \\*not italic\\* and a literal \\`backtick\\`",
    "<b>HTML is shown as text</b> & entities &amp; &lt;",
    "averyveryveryveryveryveryveryveryveryveryveryveryveryveryveryveryveryverylongwordwithoutanybreakopportunityatallinsideit",
    "مرحبا بالعالم — right-to-left text mixed with English",
    "日本語のテキストと中文和한국어",
    "Footnote-like [^1] text and a hard  \nbreak",
]

SENTENCES = [
    "Deploy finished without errors.", "Can someone review my PR?",
    "Lunch at noon?", "The build is green again.",
    "I pushed a fix for the flaky test.", "Meeting moved to 3pm.",
    "Has anyone seen the new design mocks?", "Thanks, that worked!",
    "I'll take a look after standup.", "Let's ship it.",
    "The staging server is down, investigating.", "Back online now.",
    "Here's the doc with the details.", "Agreed, let's do that.",
]


# -- HTTP ------------------------------------------------------------------


class Api:
    """A tiny Mattermost REST client on urllib."""

    def __init__(self, server: str, token: str | None = None):
        self.server = server.rstrip("/")
        self.token = token

    def request(self, method: str, path: str, body=None, *, raw: bytes | None = None,
                content_type: str | None = None, ok404: bool = False, headers=False):
        url = f"{self.server}/api/v4{path}"
        data = raw
        hdrs = {}
        if body is not None:
            data = json.dumps(body).encode()
            hdrs["Content-Type"] = "application/json"
        if content_type:
            hdrs["Content-Type"] = content_type
        if self.token:
            hdrs["Authorization"] = f"Bearer {self.token}"
        req = urllib.request.Request(url, data=data, method=method, headers=hdrs)
        for attempt in range(5):
            try:
                with urllib.request.urlopen(req, timeout=60) as resp:
                    payload = resp.read()
                    value = json.loads(payload) if payload else None
                    return (value, resp.headers) if headers else value
            except urllib.error.HTTPError as err:
                if err.code == 404 and ok404:
                    return None
                if err.code == 429 and attempt < 4:
                    time.sleep(1 + attempt)
                    continue
                detail = err.read().decode(errors="replace")
                raise SystemExit(f"{method} {path} failed: {err.code} {detail}") from None
        raise AssertionError("unreachable")

    def get(self, path, **kw):
        return self.request("GET", path, **kw)

    def post(self, path, body=None, **kw):
        return self.request("POST", path, body, **kw)

    def put(self, path, body=None, **kw):
        return self.request("PUT", path, body, **kw)

    def multipart(self, path: str, fields: dict[str, str], files: list[tuple[str, str, bytes]]):
        boundary = uuid.uuid4().hex
        parts = []
        for name, value in fields.items():
            parts.append(f"--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n".encode())
        for name, filename, data in files:
            parts.append(
                f"--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\n"
                f"Content-Type: image/png\r\n\r\n".encode() + data + b"\r\n")
        parts.append(f"--{boundary}--\r\n".encode())
        return self.post(path, raw=b"".join(parts),
                         content_type=f"multipart/form-data; boundary={boundary}")


def login(server: str, username: str, password: str = PASSWORD) -> tuple[Api, dict] | None:
    api = Api(server)
    try:
        user, hdrs = api.post("/users/login", {"login_id": username, "password": password}, headers=True)
    except SystemExit:
        return None
    api.token = hdrs["Token"]
    return api, user


# -- Images ----------------------------------------------------------------


def png(width: int, height: int, seed: int) -> bytes:
    """A deterministic two-tone diagonal pattern as a PNG."""
    rnd = random.Random(seed)
    a = bytes(rnd.randrange(40, 220) for _ in range(3))
    b = bytes(rnd.randrange(40, 220) for _ in range(3))
    band = max(8, width // 6)
    rows = bytearray()
    for y in range(height):
        rows.append(0)
        for x in range(width):
            rows += a if ((x + y) // band) % 2 == 0 else b

    def chunk(kind: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)

    header = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header)
            + chunk(b"IDAT", zlib.compress(bytes(rows), 6)) + chunk(b"IEND", b""))


# -- Seeding ---------------------------------------------------------------


def ensure_admin(server: str) -> tuple[Api, dict]:
    session = login(server, ADMIN)
    if session:
        return session
    subprocess.run(
        [str(ROOT / "scripts" / "dev-server"), "mmctl", "user", "create", "--email", "admin@example.com",
         "--username", ADMIN, "--password", PASSWORD, "--system-admin", "--email-verified"],
        check=True, stdout=subprocess.DEVNULL)
    session = login(server, ADMIN)
    if not session:
        raise SystemExit("could not sign in as the admin just created")
    return session


def ensure_user(admin: Api, username: str, first: str, last: str, index: int) -> dict:
    user = admin.get(f"/users/username/{username}", ok404=True)
    if user:
        return user
    user = admin.post("/users", {
        "username": username, "email": f"{username}@example.com", "password": PASSWORD,
        "first_name": first.capitalize(), "last_name": last.capitalize(),
        "nickname": first.capitalize(), "position": random.Random(index).choice(
            ["Engineer", "Designer", "Product", "Support", ""]),
    })
    admin.multipart(f"/users/{user['id']}/image", {}, [("image", "avatar.png", png(128, 128, index))])
    return user


def ensure_team(admin: Api, name: str, display: str) -> dict:
    team = admin.get(f"/teams/name/{name}", ok404=True)
    return team or admin.post("/teams", {"name": name, "display_name": display, "type": "O"})


def ensure_channel(admin: Api, team: dict, name: str, kind: str) -> dict:
    channel = admin.get(f"/teams/{team['id']}/channels/name/{name}?include_deleted=true", ok404=True)
    if channel:
        return channel
    display = name.replace("-", " ").title()
    return admin.post("/channels", {"team_id": team["id"], "name": name, "display_name": display,
                                    "type": kind, "purpose": f"Seeded {display.lower()} channel",
                                    "header": f"Header for **{display}** with a [link](https://mattermost.com)"})


def ensure_emoji(admin: Api, creator: dict, name: str, index: int) -> dict:
    emoji = admin.get(f"/emoji/name/{name}", ok404=True)
    if emoji:
        return emoji
    meta = json.dumps({"creator_id": creator["id"], "name": name})
    return admin.multipart("/emoji", {"emoji": meta}, [("image", f"{name}.png", png(64, 64, 1000 + index))])


def ensure_category(api: Api, user: dict, team: dict, name: str, channel_ids: list[str]) -> dict:
    cats = api.get(f"/users/{user['id']}/teams/{team['id']}/channels/categories")
    for cat in cats["categories"]:
        if cat["display_name"] == name:
            return cat
    return api.post(f"/users/{user['id']}/teams/{team['id']}/channels/categories",
                    {"user_id": user["id"], "team_id": team["id"], "display_name": name,
                     "type": "custom", "channel_ids": channel_ids})


def seeded_so_far(api: Api, channel_id: str) -> int:
    """How many seeded posts the channel holds, from the newest one's index."""
    page = api.get(f"/channels/{channel_id}/posts?per_page=60")
    indexes = [p["props"][SEED_PROP] for p in page["posts"].values()
               if SEED_PROP in (p.get("props") or {})]
    return max(indexes) + 1 if indexes else 0


def seed_channel(sessions: dict[str, Api], members: list[dict], channel: dict,
                 count: int, rnd: random.Random, names: dict) -> dict:
    """Writes `count` posts, threads and reactions into one channel."""
    result = {"written": 0, "threads": [], "images": [], "pinned": [], "markdown": []}
    admin = sessions[ADMIN]
    start = seeded_so_far(admin, channel["id"]) if count else 0
    roots: list[str] = []
    is_markdown = channel.get("name") == "markdown"
    # A rerun after a failure continues after the last seeded post.
    for n in range(start, count):
        author = rnd.choice(members)
        api = sessions[author["username"]]
        body = {"channel_id": channel["id"], "props": {SEED_PROP: n}}
        if is_markdown and n < len(MARKDOWN) * 2:
            template = MARKDOWN[n % len(MARKDOWN)]
        elif rnd.random() < 0.15:
            template = rnd.choice(MARKDOWN)
        else:
            template = " ".join(rnd.choice(SENTENCES) for _ in range(rnd.randint(1, 4)))
        body["message"] = (template.replace("{user}", rnd.choice(members)["username"])
                           .replace("{channel}", names["channel"])
                           .replace("{emoji}", rnd.choice(EMOJI)))
        if roots and rnd.random() < 0.2:
            body["root_id"] = rnd.choice(roots[-30:])
        if rnd.random() < 0.02:
            w, h = rnd.choice([(640, 480), (1200, 400), (300, 900), (64, 64)])
            upload = api.multipart("/files", {"channel_id": channel["id"]},
                                   [("files", f"image-{w}x{h}.png", png(w, h, n))])
            body["file_ids"] = [upload["file_infos"][0]["id"]]
        post = api.post("/posts", body)
        result["written"] += 1
        if "root_id" in body:
            if body["root_id"] not in result["threads"]:
                result["threads"].append(body["root_id"])
        else:
            roots.append(post["id"])
        if "file_ids" in body:
            result["images"].append(post["id"])
        if is_markdown and n < len(MARKDOWN):
            result["markdown"].append(post["id"])
        if rnd.random() < 0.15:
            for reactor in rnd.sample(members, k=min(len(members), rnd.randint(1, 4))):
                emoji = rnd.choice(["+1", "heart", "tada", "eyes", "joy", rnd.choice(EMOJI)])
                sessions[reactor["username"]].post("/reactions", {
                    "user_id": reactor["id"], "post_id": post["id"], "emoji_name": emoji})
        if n == 0 or rnd.random() < 0.01:
            admin.post(f"/posts/{post['id']}/pin")
            result["pinned"].append(post["id"])
    return result


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--server", default="http://localhost:8065")
    parser.add_argument("--users", type=int, default=20, help="users besides admin and tester")
    parser.add_argument("--posts", type=int, default=2000, help="posts across all channels")
    parser.add_argument("--seed", type=int, default=7, help="random seed")
    args = parser.parse_args()
    started = time.monotonic()
    rnd = random.Random(args.seed)

    admin_api, admin = ensure_admin(args.server)
    print(f"admin {admin['id']}")

    tester = ensure_user(admin_api, TESTER, "test", "user", 0)
    users = [admin, tester]
    for i in range(args.users):
        first, last = FIRST[i % len(FIRST)], LAST[(i * 7) % len(LAST)]
        suffix = "" if i < len(FIRST) else str(i // len(FIRST))
        users.append(ensure_user(admin_api, f"{first}.{last}{suffix}", first, last, i + 1))
    print(f"{len(users)} users")

    sessions: dict[str, Api] = {ADMIN: admin_api}
    with concurrent.futures.ThreadPoolExecutor(8) as pool:
        for user, session in zip(users[1:], pool.map(lambda u: login(args.server, u["username"]), users[1:])):
            if not session:
                raise SystemExit(f"could not sign in as {user['username']}")
            sessions[user["username"]] = session[0]

    ids = [u["id"] for u in users]
    teams = [ensure_team(admin_api, "dev", "Dev"), ensure_team(admin_api, "ops", "Ops")]
    for team in teams:
        for start in range(0, len(ids), 200):
            admin_api.post(f"/teams/{team['id']}/members/batch",
                           [{"team_id": team["id"], "user_id": uid} for uid in ids[start:start + 200]])

    emoji = [ensure_emoji(admin_api, admin, name, i) for i, name in enumerate(EMOJI)]
    print(f"{len(emoji)} custom emoji")

    channels = []  # (team name or None, channel, member list)
    for team in teams:
        for name in ["town-square", "off-topic"]:
            channels.append((team["name"], admin_api.get(f"/teams/{team['id']}/channels/name/{name}"), users))
        for name in PUBLIC_CHANNELS:
            ch = ensure_channel(admin_api, team, name, "O")
            channels.append((team["name"], ch, users))
        for i, name in enumerate(PRIVATE_CHANNELS):
            ch = ensure_channel(admin_api, team, name, "P")
            members = [admin, tester] + users[2 + i * 3: 2 + i * 3 + 5]
            channels.append((team["name"], ch, members))
        for _, ch, members in channels[-15:]:
            for member in members:
                admin_api.post(f"/channels/{ch['id']}/members", {"user_id": member["id"]})

    others = users[2:]
    for other in others[:5]:
        dm = sessions[TESTER].post("/channels/direct", [tester["id"], other["id"]])
        channels.append((None, dm, [tester, other]))
    channels.append((None, sessions[TESTER].post("/channels/direct", [tester["id"], tester["id"]]), [tester]))
    for group in [others[0:3], others[3:7]]:
        if len(group) >= 2:
            gm = sessions[TESTER].post("/channels/group", [tester["id"]] + [u["id"] for u in group])
            channels.append((None, gm, [tester] + group))
    print(f"{len(channels)} channels")

    # Posts: weighted channels first, the rest spread evenly.
    weighted = {ch["id"]: WEIGHTS.get(ch.get("name"), None) for _, ch, _ in channels}
    # A weight is the channel name's share across all teams.
    fixed = sum(w for w in weighted.values() if w is not None) / len(teams)
    rest = [cid for cid, w in weighted.items() if w is None]
    share = max(0.0, 1.0 - fixed) / max(1, len(rest))
    plan = []
    for team_name, ch, members in channels:
        weight = weighted[ch["id"]]
        count = round(args.posts * (weight / len(teams) if weight is not None else share))
        plan.append((team_name, ch, members, count))
    # Rounding leaves a few posts over or under; the busiest channel takes them.
    busiest = max(range(len(plan)), key=lambda i: plan[i][3])
    team_name, ch, members, count = plan[busiest]
    plan[busiest] = (team_name, ch, members, count + args.posts - sum(p[3] for p in plan))

    results = {}
    with concurrent.futures.ThreadPoolExecutor(8) as pool:
        futures = {
            pool.submit(seed_channel, sessions, members, ch, count,
                        random.Random(f"{args.seed}-{ch['id']}"),
                        {"channel": "town-square"}): ch["id"]
            for _, ch, members, count in plan
        }
        for fut in concurrent.futures.as_completed(futures):
            results[futures[fut]] = fut.result()
    written = sum(r["written"] for r in results.values())
    print(f"{written} posts written, {args.posts - written} were already there")

    # Sidebar categories for the tester.
    for team in teams:
        team_channels = {ch["name"]: ch["id"] for tn, ch, _, _ in plan if tn == team["name"]}
        ensure_category(sessions[TESTER], tester, team, "Projects",
                        [team_channels[n] for n in ["design", "engineering", "releases"] if n in team_channels])
        ensure_category(sessions[TESTER], tester, team, "Social",
                        [team_channels[n] for n in ["random", "music"] if n in team_channels])

    # Merge with the previous output so a rerun keeps the post ids it skipped.
    previous = {}
    if OUTPUT.exists():
        try:
            previous = json.loads(OUTPUT.read_text())
        except json.JSONDecodeError:
            previous = {}
    prev_channels = {c["id"]: c for c in previous.get("channels", [])}

    def channel_entry(team_name, ch, members, count):
        r = results[ch["id"]]
        prev = prev_channels.get(ch["id"], {})

        # Ids from earlier runs plus this one, for channels seeded in parts.
        def ids(key, new):
            return list(dict.fromkeys(prev.get(key, []) + new))

        return {
            "id": ch["id"], "team": team_name, "name": ch.get("name"), "type": ch["type"],
            "display_name": ch.get("display_name"), "members": [m["username"] for m in members],
            "posts": count,
            "thread_roots": ids("thread_roots", r["threads"]),
            "image_posts": ids("image_posts", r["images"]),
            "pinned_posts": ids("pinned_posts", r["pinned"]),
            "markdown_posts": ids("markdown_posts", r["markdown"]),
        }

    entries = [channel_entry(*p) for p in plan]
    # Saved (flagged) posts for the tester: the first thread root and image
    # post of each channel the tester is in, from the merged ids so a rerun
    # picks the same ones.
    saved = []
    for entry in entries:
        if TESTER in entry["members"]:
            saved += entry["thread_roots"][:1] + entry["image_posts"][:1]
    saved = list(dict.fromkeys(saved))[:10]
    if saved:
        sessions[TESTER].put(f"/users/{tester['id']}/preferences", [
            {"user_id": tester["id"], "category": "flagged_post", "name": pid, "value": "true"}
            for pid in saved])

    output = {
        "server": args.server,
        "password": PASSWORD,
        "admin": {"username": ADMIN, "id": admin["id"]},
        "tester": {"username": TESTER, "id": tester["id"], "mfa": False, "auth_service": ""},
        "users": [{"username": u["username"], "id": u["id"]} for u in users],
        "teams": [{"name": t["name"], "id": t["id"]} for t in teams],
        "channels": entries,
        "emoji": [{"name": e["name"], "id": e["id"]} for e in emoji],
        "saved_posts": saved,
        "markdown_samples": MARKDOWN,
    }
    OUTPUT.write_text(json.dumps(output, indent=2) + "\n")
    print(f"wrote {OUTPUT.relative_to(ROOT)} in {time.monotonic() - started:.1f}s")


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        sys.exit(130)
