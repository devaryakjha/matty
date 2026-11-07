"""Months of older history for the web demo, so its channels scroll like a
real workspace's: thousands of posts before the story in `content.py`.

Everything is generated from fixed seeds, so a rebuild gives the same
workspace. Each channel gets conversations (a few people within minutes,
some with threads and reactions) on most working days between `START` and
`END` days ago, written from the templates below: plain chat, long
multi-paragraph posts, code in several languages, long JSON and log dumps,
tables, lists, quotes and links. `seed.py` adds them to its plan.
"""

import json
import random

import content as C

START, END = 40, 200  # days ago: right before the story, back about half a year

PEOPLE = [p for p in C.EVERYONE if p != C.ME]

SERVICES = ["api", "billing", "search", "notifier", "auth", "ingest", "web", "worker"]
REGIONS = ["eu-west-1", "us-east-1", "ap-south-1"]
FEATURES = ["onboarding", "billing page", "search", "notifications", "dark mode", "the job queue",
            "SSO", "the mobile app", "exports", "the admin panel", "webhooks", "the pricing table"]
WORDS = ("latency cache index retry queue token schema migration rollout canary flag dashboard "
         "budget alert replica shard payload cursor backoff timeout quota").split()

OPENERS = [
    "Quick question about {feature}: who owns it now?",
    "Has anyone looked at {feature} on Safari lately?",
    "I'm picking up {feature} this sprint. Any context I should read first?",
    "Heads up: {service} deploys are paused until the migration lands.",
    "Morning! Is the {service} dashboard broken for anyone else?",
    "The {feature} numbers from last week look a lot better :chart_with_upwards_trend:",
    "Can I get a second pair of eyes on the {feature} PR? https://example.com/pr/{n}",
    "Reminder that {feature} goes behind a flag on Thursday.",
    "We got three tickets about {feature} today. Same root cause?",
    "Draft for {feature} is up, comments welcome: https://example.com/docs/{n}",
]
REPLIES = [
    "Looking now.", "On it :+1:", "Yes, that was me. Fixing.", "Not that I know of.",
    "I think {person} touched it last.", "Let's pair on it after lunch?", "Same here, refreshing helps for a bit.",
    "Left a few comments, mostly naming.", "Approved, ship it :rocket:", "Can we keep this in a thread?",
    "Good catch, thank you!", "I'll add it to the next planning session.", "+1, this bit me too.",
    "It's the {word} again, I'm fairly sure.", "Works for me on the latest build.",
    "Do we have a ticket for this?", "Filed https://example.com/issues/{n}", "Ha, classic :sweat_smile:",
    "Agreed. Let's not block the release on it.", "Nice work :tada:",
]
LONG = [
    "Some notes from today's sync on {feature}, so they don't get lost:\n\n"
    "1. We keep the current behaviour for existing accounts and move new ones over first.\n"
    "2. {person} owns the rollout plan, and the flag stays off in production until the {word} work is done.\n"
    "3. Support gets the macros a week before launch, not the day of.\n\n"
    "Open questions:\n\n- Do we migrate the old exports, or let them expire?\n- What do we show while the {word} is warming up?\n\n"
    "If I missed anything, reply here and I'll update the doc.",
    "Long one, sorry. I spent the afternoon on why {service} gets slow every evening.\n\n"
    "The short version: the nightly report job and the evening traffic peak overlap, and both hit the same replica. "
    "The report job does a full scan of the events table, which pushes the hot pages for user lookups out of the cache. "
    "Once that happens every request pays for a disk read, p99 goes from 80 ms to about 900 ms, and the retries make it worse.\n\n"
    "Options, roughly in order of effort:\n\n"
    "- Move the report job to 03:00, when traffic is lowest.\n"
    "- Point it at the analytics replica, which nobody serves traffic from.\n"
    "- Make the report incremental, so it only reads the last day.\n\n"
    "I'd do the first two this week and the third next quarter. Thoughts?",
    "> {quote}\n\nI keep coming back to this. We wrote it in the very first design doc and it still holds: "
    "if a screen needs a manual, the screen is wrong. For {feature} that means fewer settings, better defaults, "
    "and an empty state that tells you what to do next.",
]
QUOTES = ["Make the common case fast and the rare case possible.",
          "Defaults are the product.", "Measure first, then optimise.",
          "Every alert should be actionable."]

CODE = [
    ("rust", "pub fn backoff(attempt: u32) -> Duration {{\n    let base = Duration::from_millis(250);\n"
             "    let capped = attempt.min(6);\n    base * 2u32.pow(capped) + jitter(base)\n}}"),
    ("typescript", "export async function fetchWithRetry(url: string, tries = 3): Promise<Response> {{\n"
                   "  for (let i = 0; i < tries; i++) {{\n    const res = await fetch(url);\n"
                   "    if (res.ok || res.status < 500) return res;\n    await sleep(2 ** i * 250);\n  }}\n"
                   "  throw new Error(`gave up on ${{url}}`);\n}}"),
    ("sql", "SELECT date_trunc('day', created_at) AS day, count(*) AS signups\nFROM accounts\n"
            "WHERE created_at > now() - interval '30 days'\nGROUP BY 1\nORDER BY 1;"),
    ("python", "def p99(samples):\n    ordered = sorted(samples)\n    return ordered[int(len(ordered) * 0.99) - 1]"),
    ("yaml", "service: {service}\nreplicas: 3\nresources:\n  cpu: 500m\n  memory: 512Mi\n"
             "env:\n  - name: CACHE_TTL\n    value: \"300\""),
    ("bash", "$ kubectl -n prod rollout restart deploy/{service}\n"
             "deployment.apps/{service} restarted\n$ kubectl -n prod rollout status deploy/{service}\n"
             "deployment \"{service}\" successfully rolled out"),
    ("go", "func (c *Client) Do(req *http.Request) (*http.Response, error) {{\n"
           "\tctx, cancel := context.WithTimeout(req.Context(), c.timeout)\n\tdefer cancel()\n"
           "\treturn c.http.Do(req.WithContext(ctx))\n}}"),
]


def json_dump(rnd, n):
    """A long, pretty-printed JSON payload, the kind people paste when
    something looks wrong."""
    events = [{
        "id": f"evt_{rnd.randrange(16**10):010x}",
        "service": rnd.choice(SERVICES),
        "region": rnd.choice(REGIONS),
        "status": rnd.choice([200, 200, 200, 429, 500, 503]),
        "latency_ms": rnd.randint(12, 2400),
        "attempt": rnd.randint(1, 4),
        "tags": rnd.sample(WORDS, 3),
        "request": {"method": rnd.choice(["GET", "POST", "PATCH"]), "path": f"/v2/{rnd.choice(WORDS)}s/{rnd.randint(1, 99999)}",
                    "headers": {"x-request-id": f"{rnd.randrange(16**16):016x}", "user-agent": "lumen-sdk/2.3.1"}},
    } for _ in range(n)]
    return json.dumps({"query": "status >= 429", "count": n, "events": events}, indent=2)


def log_dump(rnd, n):
    lines = []
    for i in range(n):
        level = rnd.choice(["INFO", "INFO", "INFO", "WARN", "ERROR"])
        svc = rnd.choice(SERVICES)
        lines.append(f"2026-06-{rnd.randint(1, 28):02d}T{rnd.randint(0, 23):02d}:{rnd.randint(0, 59):02d}:{rnd.randint(0, 59):02d}Z "
                     f"{level:5} {svc}: {rnd.choice(WORDS)} {rnd.choice(['ok', 'slow', 'retrying', 'timed out', 'evicted'])} "
                     f"in {rnd.randint(1, 3000)}ms (attempt {rnd.randint(1, 5)}, shard {rnd.randint(0, 15)})")
    return "\n".join(lines)


def table(rnd):
    rows = "\n".join(f"| {s} | {rnd.randint(20, 400)} ms | {rnd.randint(300, 4000)} ms | {rnd.uniform(0, 2):.2f}% |"
                     for s in rnd.sample(SERVICES, 4))
    return f"Weekly latency report:\n\n| Service | p50 | p99 | Errors |\n| :--- | ---: | ---: | ---: |\n{rows}"


def fill(rnd, template):
    return template.format(
        feature=rnd.choice(FEATURES), service=rnd.choice(SERVICES), person="@" + rnd.choice(PEOPLE),
        word=rnd.choice(WORDS), n=rnd.randint(1000, 9999), quote=rnd.choice(QUOTES))


def opener(rnd):
    roll = rnd.random()
    if roll < 0.55:
        return fill(rnd, rnd.choice(OPENERS))
    if roll < 0.68:
        return fill(rnd, rnd.choice(LONG))
    if roll < 0.80:
        lang, code = rnd.choice(CODE)
        return f"{fill(rnd, 'Here is what I have for {feature}:')}\n\n```{lang}\n{fill(rnd, code)}\n```"
    if roll < 0.87:
        return f"These are the failing requests from the last hour:\n\n```json\n{json_dump(rnd, rnd.randint(3, 12))}\n```"
    if roll < 0.93:
        return f"Log excerpt from {rnd.choice(SERVICES)}:\n\n```\n{log_dump(rnd, rnd.randint(8, 40))}\n```"
    return table(rnd)


def conversation(rnd, days, people):
    hour, minute = rnd.randint(8, 18), rnd.randint(0, 59)
    first, *others = rnd.sample(people, k=min(len(people), rnd.randint(2, 5)))
    root = {"by": first, "at": (days, f"{hour:02d}:{minute:02d}"), "text": opener(rnd)}
    if rnd.random() < 0.35:
        root["react"] = {rnd.choice(["+1", "eyes", "tada", "white_check_mark", "rocket", "100"]):
                         rnd.sample(people, k=rnd.randint(1, 3))}
    posts = [root]
    if rnd.random() < 0.3:
        # Discussed in a thread.
        root["replies"] = [{"by": rnd.choice(others or [first]), "at": 2 + i * rnd.randint(1, 9),
                            "text": fill(rnd, rnd.choice(REPLIES))} for i in range(rnd.randint(1, 6))]
    else:
        for i, person in enumerate(others):
            minute = min(59, minute + rnd.randint(1, 6))
            posts.append({"by": person, "at": (days, f"{hour:02d}:{minute:02d}"),
                          "text": fill(rnd, rnd.choice(REPLIES))})
    return posts


# Channel name -> conversations on an average working day.
BUSY = {
    "town-square": 1.5, "off-topic": 2, "engineering": 4, "frontend": 2.5, "design": 2,
    "incidents": 1, "help-desk": 2, "launch-v2": 1.5,
}


def history(members):
    """(team, channel) -> older posts. `members` maps a channel name to who
    is in it (None for everyone)."""
    plan = {}
    for name, rate in BUSY.items():
        rnd = random.Random(f"history-{name}")
        people = [p for p in (members.get(name) or C.EVERYONE)]
        posts = []
        for days in range(START, END):
            weekday = days % 7 not in (5, 6)
            for _ in range(rnd.randint(0, round(rate * (2 if weekday else 0.4)))):
                posts.extend(conversation(rnd, days, people))
        plan[("lumen", name)] = posts
    return plan
