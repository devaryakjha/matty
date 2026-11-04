#!/usr/bin/env python3
"""Writes the web demo's sample workspace (content.py) into the dev server.

    scripts/dev-server reset && scripts/demo/seed.py

Run it on a fresh server: it creates everything and does not look for what
is already there. Posts get timestamps over the last three weeks (each
author is a system admin while seeding so the server keeps `create_at`,
then goes back to a normal user). `scripts/demo/run` runs this and snapshots
the result. Needs Pillow for the images (pip install pillow).
"""

from __future__ import annotations

import argparse
import concurrent.futures
import datetime as dt
import io
import json
import random
import subprocess
import sys
import urllib.error
import urllib.request
import uuid
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

sys.path.insert(0, str(Path(__file__).resolve().parent))
import content as C  # noqa: E402

PASSWORD = "Password1!"
ADMIN = "admin"


class Api:
    def __init__(self, server: str, token: str | None = None):
        self.server = server.rstrip("/")
        self.token = token

    def request(self, method, path, body=None, *, raw=None, content_type=None, headers=False):
        data = raw
        hdrs = {}
        if body is not None:
            data = json.dumps(body).encode()
            hdrs["Content-Type"] = "application/json"
        if content_type:
            hdrs["Content-Type"] = content_type
        if self.token:
            hdrs["Authorization"] = f"Bearer {self.token}"
        req = urllib.request.Request(f"{self.server}/api/v4{path}", data=data, method=method, headers=hdrs)
        try:
            with urllib.request.urlopen(req, timeout=60) as resp:
                payload = resp.read()
                value = json.loads(payload) if payload else None
                return (value, resp.headers) if headers else value
        except urllib.error.HTTPError as err:
            raise SystemExit(f"{method} {path} failed: {err.code} {err.read().decode(errors='replace')}") from None

    def get(self, path):
        return self.request("GET", path)

    def post(self, path, body=None, **kw):
        return self.request("POST", path, body, **kw)

    def put(self, path, body=None):
        return self.request("PUT", path, body)

    def upload(self, path, fields, files):
        """multipart/form-data with `files` as (field, filename, bytes, type)."""
        boundary = uuid.uuid4().hex
        parts = [f"--{boundary}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n".encode()
                 for k, v in fields.items()]
        for field, name, data, kind in files:
            parts.append(f"--{boundary}\r\nContent-Disposition: form-data; name=\"{field}\"; "
                         f"filename=\"{name}\"\r\nContent-Type: {kind}\r\n\r\n".encode() + data + b"\r\n")
        parts.append(f"--{boundary}--\r\n".encode())
        return self.post(path, raw=b"".join(parts), content_type=f"multipart/form-data; boundary={boundary}")


def ensure_admin(server):
    try:
        return login(server, ADMIN)
    except SystemExit:
        root = Path(__file__).resolve().parents[2]
        subprocess.run([str(root / "scripts" / "dev-server"), "mmctl", "user", "create", "--email", "admin@lumen.example",
                        "--username", ADMIN, "--password", PASSWORD, "--system-admin", "--email-verified"],
                       check=True, stdout=subprocess.DEVNULL)
        return login(server, ADMIN)


def login(server, username):
    api = Api(server)
    _, hdrs = api.post("/users/login", {"login_id": username, "password": PASSWORD}, headers=True)
    api.token = hdrs["Token"]
    return api


# -- Generated images and files ---------------------------------------------


def font(size):
    try:
        return ImageFont.load_default(size=size)
    except TypeError:
        return ImageFont.load_default()


def png(image):
    out = io.BytesIO()
    image.save(out, "PNG", optimize=True)
    return out.getvalue()


def rgb(hex_color):
    hex_color = hex_color.lstrip("#")
    return tuple(int(hex_color[i:i + 2], 16) for i in (0, 2, 4))


def avatar(first, last, color):
    size = 256
    image = Image.new("RGB", (size, size), rgb(color))
    draw = ImageDraw.Draw(image)
    initials = (first[0] + last[0]).upper()
    draw.text((size / 2, size / 2), initials, fill="white", font=font(110), anchor="mm")
    return png(image)


def emoji_image(name, index):
    colors = ["#16a34a", "#2563eb", "#f59e0b", "#db2777", "#ef4444", "#7c3aed"]
    image = Image.new("RGBA", (128, 128), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)
    draw.rounded_rectangle((4, 4, 124, 124), radius=28, fill=rgb(colors[index % len(colors)]))
    label = {"shipit": "SHIP", "lgtm": "LGTM", "lumen": "L", "coffee-parrot": "☕",
             "this-is-fine": "🔥", "big-brain": "IQ"}.get(name, name[:2].upper())
    draw.text((64, 66), label, fill="white", font=font(44 if len(label) > 2 else 64), anchor="mm")
    return png(image)


def mockup(dark=False, title="Lumen", accent="#7c3aed"):
    w, h = 1280, 800
    bg, panel, line, text = (("#0f172a", "#1e293b", "#334155", "#e2e8f0") if dark
                             else ("#f8fafc", "#ffffff", "#e2e8f0", "#0f172a"))
    image = Image.new("RGB", (w, h), rgb(bg))
    d = ImageDraw.Draw(image)
    d.rectangle((0, 0, 260, h), fill=rgb(panel))
    d.line((260, 0, 260, h), fill=rgb(line), width=2)
    d.text((28, 32), title, fill=rgb(text), font=font(34))
    for i, name in enumerate(["Launch v2", "Design", "Engineering", "Standup", "Incidents", "Off-Topic"]):
        y = 110 + i * 52
        if i == 0:
            d.rounded_rectangle((16, y - 10, 244, y + 34), radius=10, fill=rgb(accent))
        d.text((32, y), f"# {name}", fill="white" if i == 0 else rgb(text), font=font(24))
    for i in range(6):
        y = 60 + i * 118
        d.ellipse((300, y, 356, y + 56), fill=rgb(["#2563eb", "#db2777", "#059669", "#ea580c", "#ca8a04", "#0891b2"][i]))
        d.rounded_rectangle((376, y + 4, 376 + 180, y + 24), radius=6, fill=rgb(line))
        d.rounded_rectangle((376, y + 36, 376 + 480 + (i * 97) % 300, y + 56), radius=6, fill=rgb(line))
    d.rounded_rectangle((300, h - 110, w - 40, h - 36), radius=16, outline=rgb(line), width=3, fill=rgb(panel))
    d.text((330, h - 84), "Message #launch-v2", fill=rgb("#94a3b8"), font=font(24))
    return png(image)


def settings_mockup():
    w, h = 1280, 800
    image = Image.new("RGB", (w, h), rgb("#f8fafc"))
    d = ImageDraw.Draw(image)
    d.text((80, 60), "Settings", fill=rgb("#0f172a"), font=font(48))
    for i, (label, value) in enumerate([("Theme", "System"), ("Text size", "100%"), ("Notifications", "Mentions"),
                                        ("Send messages on", "Enter"), ("Show link previews", "On")]):
        y = 170 + i * 110
        d.rounded_rectangle((80, y, w - 80, y + 86), radius=14, fill="white", outline=rgb("#e2e8f0"), width=2)
        d.text((112, y + 28), label, fill=rgb("#0f172a"), font=font(28))
        d.rounded_rectangle((w - 340, y + 20, w - 112, y + 66), radius=10, fill=rgb("#ede9fe"))
        d.text((w - 226, y + 43), value, fill=rgb("#5b21b6"), font=font(24), anchor="mm")
    return png(image)


def chart():
    w, h = 1200, 640
    image = Image.new("RGB", (w, h), "white")
    d = ImageDraw.Draw(image)
    d.text((60, 30), "Signups per week", fill=rgb("#0f172a"), font=font(36))
    values = [120, 160, 150, 210, 260, 240, 330, 410, 480, 560, 690, 820]
    top, bottom, left = 110, h - 80, 90
    d.line((left, bottom, w - 40, bottom), fill=rgb("#94a3b8"), width=2)
    step = (w - left - 60) / len(values)
    for i, value in enumerate(values):
        x = left + 20 + i * step
        y = bottom - (bottom - top) * value / max(values)
        d.rounded_rectangle((x, y, x + step * 0.6, bottom), radius=8, fill=rgb("#7c3aed"))
        d.text((x + step * 0.3, bottom + 24), f"W{i + 1}", fill=rgb("#64748b"), font=font(20), anchor="mm")
        d.text((x + step * 0.3, y - 18), str(value), fill=rgb("#334155"), font=font(18), anchor="mm")
    return png(image)


def poster(title, top, bottom):
    w, h = 1200, 800
    image = Image.new("RGB", (w, h))
    d = ImageDraw.Draw(image)
    a, b = rgb(top), rgb(bottom)
    for y in range(h):
        t = y / h
        d.line((0, y, w, y), fill=tuple(int(a[i] + (b[i] - a[i]) * t) for i in range(3)))
    d.text((w / 2, h / 2), title, fill="white", font=font(96), anchor="mm")
    d.text((w / 2, h / 2 + 100), "Coming soon", fill="white", font=font(40), anchor="mm")
    return png(image)


def plant():
    w, h = 900, 900
    image = Image.new("RGB", (w, h), rgb("#fef3c7"))
    d = ImageDraw.Draw(image)
    d.polygon([(330, 900), (570, 900), (610, 640), (290, 640)], fill=rgb("#b45309"))
    rnd = random.Random(3)
    for i in range(9):
        cx, cy = 450 + rnd.randint(-260, 260), 360 + rnd.randint(-250, 180)
        d.line((450, 640, cx, cy), fill=rgb("#166534"), width=10)
        d.ellipse((cx - 110, cy - 70, cx + 110, cy + 70), fill=rgb(["#16a34a", "#15803d", "#22c55e"][i % 3]))
    return png(image)


def onboarding():
    w, h = 1280, 800
    image = Image.new("RGB", (w, h), rgb("#f1f5f9"))
    d = ImageDraw.Draw(image)
    for i, step in enumerate(["Your name", "Your team", "Invite people"]):
        x = 120 + i * 360
        d.rounded_rectangle((x, 160, x + 320, 640), radius=24, fill="white", outline=rgb("#cbd5e1"), width=2)
        d.ellipse((x + 130, 210, x + 190, 270), fill=rgb("#7c3aed" if i == 1 else "#cbd5e1"))
        d.text((x + 160, 240), str(i + 1), fill="white", font=font(30), anchor="mm")
        d.text((x + 160, 330), step, fill=rgb("#0f172a"), font=font(32), anchor="mm")
        d.rounded_rectangle((x + 40, 400, x + 280, 450), radius=10, fill=rgb("#f1f5f9"))
        d.rounded_rectangle((x + 40, 540, x + 280, 596), radius=12, fill=rgb("#7c3aed"))
        d.text((x + 160, 568), "Continue", fill="white", font=font(24), anchor="mm")
    return png(image)


def pdf(title, lines):
    """A one-page PDF with a title and some lines of text."""
    text = ["BT /F1 24 Tf 72 740 Td (" + title + ") Tj ET"]
    for i, line in enumerate(lines):
        text.append(f"BT /F1 13 Tf 72 {700 - i * 22} Td ({line}) Tj ET")
    stream = "\n".join(text).encode()
    objects = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>",
        b"<< /Length " + str(len(stream)).encode() + b" >>\nstream\n" + stream + b"\nendstream",
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    ]
    out = io.BytesIO()
    out.write(b"%PDF-1.4\n")
    offsets = []
    for number, body in enumerate(objects, 1):
        offsets.append(out.tell())
        out.write(f"{number} 0 obj\n".encode() + body + b"\nendobj\n")
    xref = out.tell()
    out.write(f"xref\n0 {len(objects) + 1}\n0000000000 65535 f \n".encode())
    for offset in offsets:
        out.write(f"{offset:010d} 00000 n \n".encode())
    out.write(f"trailer\n<< /Size {len(objects) + 1} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n".encode())
    return out.getvalue()


FILES = {
    "mockup-home.png": (lambda: mockup(), "image/png"),
    "mockup-settings.png": (settings_mockup, "image/png"),
    "dark-mode.png": (lambda: mockup(dark=True), "image/png"),
    "signups-chart.png": (chart, "image/png"),
    "press-kit-cover.png": (lambda: poster("Lumen 2.0", "#7c3aed", "#db2777"), "image/png"),
    "plant.png": (plant, "image/png"),
    "onboarding.png": (onboarding, "image/png"),
    "launch-plan.pdf": (lambda: pdf("Lumen 2.0 launch plan", [
        "Launch: five weeks from kickoff", "Code freeze: one week before launch", "Owners: Aiko (PM), Theo (Eng), Maya (Design)",
        "Risks: billing migration, store review times"]), "application/pdf"),
    "pricing.csv": (lambda: b"plan,seats,price_usd,trial_days\nstarter,10,0,0\nteam,50,8,14\nbusiness,500,14,30\n", "text/csv"),
    "release-notes.md": (lambda: b"# Lumen 2.0\n\n- New sidebar\n- Threads everywhere\n- Dark mode\n- 40% faster startup\n",
                         "text/markdown"),
    "postmortem-api-errors.md": (lambda: b"# Post-mortem: elevated API errors\n\n## Impact\n\n12 minutes of 3% errors in eu-west.\n\n"
                                 b"## Root cause\n\nA kernel update broke one node's network driver.\n\n## Actions\n\n- Health checks probe the network path\n",
                                 "text/markdown"),
}

# Older everyday chatter, so channels scroll back further than the story.
FILLER = {
    ("lumen", "engineering"): [
        "Merged the logging cleanup, logs are about half the size now.", "Who owns the flaky e2e test on checkout?",
        "Rotated the staging certificates.", "Can someone review the rate limiter PR? It's small, promise.",
        "Bumped the Node version in CI.", "Heads up: the staging DB restarts at 14:00.",
        "Docs for the webhooks API are updated.", "Feature flags are now in the admin panel.",
        "The search index rebuild finished overnight.", "Pairing session on the queue at 15:00, join if curious.",
    ],
    ("lumen", "town-square"): [
        "Reminder: timesheets are due Friday.", "Coffee machine on 3 is fixed :coffee:",
        "The office is closed on Monday for maintenance.", "Lunch and learn on Thursday: accessibility basics.",
        "New hire onboarding docs are in the wiki.", "Happy Friday everyone!",
    ],
    ("lumen", "off-topic"): [
        "Anyone have a good podcast for the commute?", "There's cake in the kitchen :birthday:",
        "Rainy day playlist recommendations welcome.", "Board game night next week?",
        "Spotted a fox outside the office this morning :fox_face:", "Best keyboard switches, go.",
    ],
    ("lumen", "design"): [
        "Updated the color tokens in the library.", "Who has the latest logo files?", "Typography scale is final.",
        "Running a quick usability test tomorrow, need two volunteers.",
    ],
}


# Matty draws emoji shortcodes in reactions but not yet in message text
# (#103), so the seed writes the glyphs, as people who type them do.
GLYPHS = {
    "+1": "👍", "100": "💯", "airplane": "✈️", "birthday": "🎂", "books": "📚", "camera": "📷",
    "chart_with_upwards_trend": "📈", "coffee": "☕", "fire": "🔥", "fox_face": "🦊", "green_circle": "🟢",
    "herb": "🌿", "large_green_circle": "🟢", "large_yellow_circle": "🟡", "mega": "📣", "musical_note": "🎵",
    "pencil2": "✏️", "point_down": "👇", "point_up": "👆", "pray": "🙏", "ramen": "🍜", "rocket": "🚀",
    "rotating_light": "🚨", "snowflake": "❄️", "sparkles": "✨", "sunrise": "🌅", "sweat_smile": "😅",
    "tada": "🎉", "wave": "👋", "white_check_mark": "✅", "wink": "😉", "zap": "⚡",
}


def glyphs(text):
    for name, glyph in GLYPHS.items():
        text = text.replace(f":{name}:", glyph)
    return text


# -- Seeding -----------------------------------------------------------------


class Seeder:
    def __init__(self, server):
        self.server = server
        self.now = dt.datetime.now(dt.timezone.utc).replace(second=0, microsecond=0)
        self.admin = ensure_admin(server)
        self.users = {}
        self.sessions = {}
        self.channels = {}  # (team, name) or ("dm", username) -> channel id
        self.teams = {}
        self.uploads = {}
        self.saved = []

    def at(self, days, clock):
        hour, minute = map(int, clock.split(":"))
        day = self.now - dt.timedelta(days=days)
        when = day.replace(hour=hour, minute=minute)
        # Nothing in the story is in the future.
        return int(min(when, self.now - dt.timedelta(minutes=30)).timestamp() * 1000)

    def people(self):
        def make(item):
            username, (first, last, position, color) = item
            user = self.admin.post("/users", {
                "username": username, "email": f"{username}@lumen.example", "password": PASSWORD,
                "first_name": first, "last_name": last, "nickname": first, "position": position})
            self.admin.upload(f"/users/{user['id']}/image", {}, [("image", "avatar.png", avatar(first, last, color), "image/png")])
            # Admin while seeding, so the server keeps the posts' timestamps.
            self.admin.put(f"/users/{user['id']}/roles", {"roles": "system_user system_admin"})
            return username, user, login(self.server, username)

        with concurrent.futures.ThreadPoolExecutor(8) as pool:
            for username, user, session in pool.map(make, C.PEOPLE.items()):
                self.users[username] = user
                self.sessions[username] = session
        print(f"{len(self.users)} people")
        for index, name in enumerate(C.CUSTOM_EMOJI):
            meta = json.dumps({"creator_id": self.users["lena"]["id"], "name": name})
            self.sessions["lena"].upload("/emoji", {"emoji": meta}, [("image", f"{name}.png", emoji_image(name, index), "image/png")])

    def workspace(self):
        everyone = [self.users[name]["id"] for name in C.PEOPLE]
        for team_name, (display, channels) in C.TEAMS.items():
            team = self.admin.post("/teams", {"name": team_name, "display_name": display, "type": "O"})
            self.teams[team_name] = team
            self.admin.post(f"/teams/{team['id']}/members/batch", [{"team_id": team["id"], "user_id": uid} for uid in everyone])
            for name, title, kind, purpose, header, members in channels:
                if name in ("town-square", "off-topic"):
                    channel = self.admin.get(f"/teams/{team['id']}/channels/name/{name}")
                    self.admin.put(f"/channels/{channel['id']}/patch", {"purpose": purpose, "header": header})
                else:
                    channel = self.admin.post("/channels", {"team_id": team["id"], "name": name, "display_name": title,
                                                            "type": kind, "purpose": purpose, "header": header})
                for username in members or C.PEOPLE:
                    self.admin.post(f"/channels/{channel['id']}/members", {"user_id": self.users[username]["id"]})
                self.channels[(team_name, name)] = channel["id"]
        me = self.sessions[C.ME]
        my_id = self.users[C.ME]["id"]
        for other in C.DIRECT:
            channel = me.post("/channels/direct", [my_id, self.users[other]["id"]])
            self.channels[("dm", other)] = channel["id"]
        self.channels[("dm", C.ME)] = me.post("/channels/direct", [my_id, my_id])["id"]
        group, _ = C.GROUP
        self.channels[("group",)] = me.post("/channels/group", [my_id] + [self.users[u]["id"] for u in group])["id"]
        print(f"{len(self.channels)} channels")

    def file_ids(self, author, channel_id, names):
        ids = []
        for name in names:
            make, kind = FILES[name]
            data = make()
            upload = self.sessions[author].upload("/files", {"channel_id": channel_id}, [("files", name, data, kind)])
            ids.append(upload["file_infos"][0]["id"])
        return ids

    def write(self, channel_id, post, root=None, root_at=None):
        author = post["by"]
        if root:
            when = root_at + post["at"] * 60_000
        elif "at" in post:
            when = self.at(*post["at"])
        else:
            when = None
        props = json.loads(glyphs(json.dumps(post.get("props", {}), ensure_ascii=False)))
        body = {"channel_id": channel_id, "message": glyphs(post.get("text", "")), "props": props}
        if when:
            body["create_at"] = when
        if root:
            body["root_id"] = root
        if post.get("files"):
            body["file_ids"] = self.file_ids(author, channel_id, post["files"])
        created = self.sessions[author].post("/posts", body)
        for emoji, people in post.get("react", {}).items():
            for person in people:
                self.sessions[person].post("/reactions", {"user_id": self.users[person]["id"],
                                                          "post_id": created["id"], "emoji_name": emoji})
        if post.get("pin"):
            self.admin.post(f"/posts/{created['id']}/pin")
        if post.get("save"):
            self.saved.append(created["id"])
        if post.get("edit"):
            self.sessions[author].put(f"/posts/{created['id']}/patch", {"message": glyphs(post["edit"])})
        for reply in post.get("replies", []):
            self.write(channel_id, reply, root=created["id"], root_at=when)
        return created

    def posts(self):
        plan = [
            (("lumen", "formatting-tour"), C.FORMATTING), (("lumen", "launch-v2"), C.LAUNCH),
            (("lumen", "design"), C.DESIGN), (("lumen", "engineering"), C.ENGINEERING),
            (("lumen", "frontend"), C.FRONTEND), (("lumen", "incidents"), C.INCIDENTS),
            (("lumen", "help-desk"), C.HELP_DESK), (("lumen", "town-square"), C.TOWN_SQUARE),
            (("lumen", "off-topic"), C.OFF_TOPIC), (("lumen", "design-crit"), C.DESIGN_CRIT),
            (("lumen", "launch-war-room"), C.WAR_ROOM), (("lumen", "standup"), self.standups()),
            (("dm", C.ME), C.SELF_NOTES), (("group",), C.GROUP[1]),
        ]
        plan += [(("community", name), posts) for name, posts in C.COMMUNITY.items()]
        plan += [(("dm", other), posts) for other, posts in C.DIRECT.items()]
        plan += [(key, self.filler(key, lines)) for key, lines in FILLER.items()]
        with concurrent.futures.ThreadPoolExecutor(8) as pool:
            futures = [pool.submit(self.channel_posts, self.channels[key], posts) for key, posts in plan]
            written = sum(f.result() for f in futures)
        print(f"{written} posts")

    def channel_posts(self, channel_id, posts):
        count = 0
        for post in sorted(posts, key=lambda p: (-p["at"][0], p["at"][1])):
            self.write(channel_id, post)
            count += 1 + len(post.get("replies", []))
        return count

    def standups(self):
        rnd = random.Random(11)
        posts = []
        for days in range(0, 22):
            if (self.now - dt.timedelta(days=days)).weekday() >= 5:
                continue
            for i, person in enumerate(rnd.sample(C.STANDUP_PEOPLE, k=5)):
                yesterday = rnd.choice(C.STANDUP_LINES["yesterday"])
                today = rnd.choice(C.STANDUP_LINES["today"])
                blockers = rnd.choice(["None", "None", "None", "Waiting on a review", "Need design input on empty states"])
                posts.append({"by": person, "at": (days, f"09:{5 + i * 7:02d}"),
                              "text": f"**Yesterday:** {yesterday}\n**Today:** {today}\n**Blockers:** {blockers}"})
        return posts

    def filler(self, key, lines):
        rnd = random.Random(str(key))
        people = [p for p in C.EVERYONE if p != C.ME]
        posts = []
        for days in range(22, 40):
            for _ in range(rnd.randint(1, 3)):
                posts.append({"by": rnd.choice(people), "at": (days, f"{rnd.randint(9, 17):02d}:{rnd.randint(0, 59):02d}"),
                              "text": rnd.choice(lines)})
        return posts

    def finish(self):
        me = self.sessions[C.ME]
        my_id = self.users[C.ME]["id"]
        if self.saved:
            me.put(f"/users/{my_id}/preferences", [
                {"user_id": my_id, "category": "flagged_post", "name": pid, "value": "true"} for pid in self.saved])
        me.put(f"/users/{my_id}/preferences", [
            {"user_id": my_id, "category": "favorite_channel", "name": self.channels[key], "value": "true"}
            for key in C.FAVORITES])
        for (team, name), channels in C.CATEGORIES.items():
            team_id = self.teams[team]["id"]
            me.post(f"/users/{my_id}/teams/{team_id}/channels/categories", {
                "user_id": my_id, "team_id": team_id, "display_name": name, "type": "custom",
                "channel_ids": [self.channels[(team, c)] for c in channels]})
        # Everything so far is read; what follows is new.
        for channel_id in self.channels.values():
            me.post(f"/channels/members/{my_id}/view", {"channel_id": channel_id})
        for key, post in C.UNREAD:
            self.write(self.channels[key], post)
        for username, user in self.users.items():
            self.admin.put(f"/users/{user['id']}/roles", {"roles": "system_user"})
        print(f"{len(self.saved)} saved, {len(C.UNREAD)} unread")


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--server", default="http://localhost:8065")
    args = parser.parse_args()
    seeder = Seeder(args.server)
    seeder.people()
    seeder.workspace()
    seeder.posts()
    seeder.finish()
    print(f"sign in as {C.ME} / {PASSWORD}")


if __name__ == "__main__":
    main()
