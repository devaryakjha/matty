"""The web demo's sample workspace: people, channels and conversations.

Lumen Labs is a made-up company of twelve people building a product launch.
Every name, company and link here is fictional. `seed.py` writes it into a
dev server with real timestamps over the last three weeks; `scripts/demo/run`
then snapshots it for https://matty.jha.sh.

A post is a dict:
  by       username of the author
  at       (days ago, "HH:MM"); replies may give minutes after the root
  text     the message (Markdown)
  replies  list of posts (each `at` is minutes after the root)
  react    {emoji: [usernames]}
  pin      pinned in the channel
  save     saved by the demo user
  files    names of generated files (see seed.py FILES)
  edit     the message it was edited to later
  props    extra post props (message attachments)
"""

ME = "maya"

# username: (first, last, position, avatar color)
PEOPLE = {
    "maya": ("Maya", "Chen", "Product Designer", "#7c3aed"),
    "theo": ("Theo", "Brandt", "Engineering Lead", "#2563eb"),
    "priya": ("Priya", "Raman", "Backend Engineer", "#db2777"),
    "sam": ("Sam", "Okafor", "Frontend Engineer", "#ea580c"),
    "lena": ("Lena", "Fischer", "Design Lead", "#059669"),
    "jonas": ("Jonas", "Weber", "Site Reliability", "#0891b2"),
    "aiko": ("Aiko", "Tanaka", "Product Manager", "#ca8a04"),
    "diego": ("Diego", "Alvarez", "Customer Support", "#dc2626"),
    "nora": ("Nora", "Lindqvist", "Marketing", "#9333ea"),
    "ravi": ("Ravi", "Patel", "Data Analyst", "#4f46e5"),
    "elif": ("Elif", "Kaya", "Mobile Engineer", "#0d9488"),
    "marcus": ("Marcus", "Hale", "CEO", "#475569"),
    "lumen-ci": ("Lumen", "CI", "Build bot", "#16a34a"),
}

EVERYONE = [name for name in PEOPLE if name != "lumen-ci"]

CUSTOM_EMOJI = ["shipit", "lgtm", "lumen", "coffee-parrot", "this-is-fine", "big-brain"]

# Team name: (display name, channels). A channel is
# (name, display name, type "O"/"P", purpose, header, members or None for all).
TEAMS = {
    "lumen": ("Lumen Labs", [
        ("town-square", "Town Square", "O", "Company-wide announcements and anything everyone should see", "", None),
        ("off-topic", "Off-Topic", "O", "Pets, plants, playlists and the occasional meme", "", None),
        ("launch-v2", "Launch v2", "O", "Everything for the Lumen 2.0 launch",
         "Launch in two weeks · Plan: https://example.com/lumen/launch-plan · Owner: @aiko", None),
        ("design", "Design", "O", "Critique, mockups and the design system",
         "Mockups live in the Lumen 2.0 project. Crit is Thursdays at 14:00.", None),
        ("engineering", "Engineering", "O", "Architecture, reviews and dev tooling",
         "On call this week: @jonas · Runbooks: https://example.com/lumen/runbooks", None),
        ("frontend", "Frontend", "O", "Web and desktop client work", "", ["maya", "sam", "theo", "lena", "elif", "aiko"]),
        ("incidents", "Incidents", "O", "Live incidents and post-mortems. Keep chatter in threads.",
         "Declare with /incident · Status page: https://status.example.com", None),
        ("standup", "Standup", "O", "Async standup: yesterday, today, blockers", "", None),
        ("formatting-tour", "Formatting Tour", "O", "Every kind of message Matty can draw, in one place",
         "Scroll up from the bottom for the whole tour", None),
        ("help-desk", "Help Desk", "O", "Customer questions the support team wants a second pair of eyes on", "", None),
        ("design-crit", "Design Crit", "P", "Honest feedback, kindly delivered", "", ["maya", "lena", "sam", "aiko"]),
        ("launch-war-room", "Launch War Room", "P", "Go/no-go and launch-day coordination", "",
         ["maya", "theo", "aiko", "marcus", "jonas", "nora"]),
    ]),
    "community": ("Open Source", [
        ("town-square", "Town Square", "O", "Say hi to the Lumen community", "", None),
        ("off-topic", "Off-Topic", "O", "", "", None),
        ("show-and-tell", "Show and Tell", "O", "Share what you built with Lumen", "", None),
        ("plugins", "Plugins", "O", "Writing and sharing Lumen plugins", "", None),
    ]),
}

# Channels in the demo user's favorites, and a custom sidebar category.
FAVORITES = [("lumen", "launch-v2"), ("lumen", "design")]
CATEGORIES = {("lumen", "Projects"): ["formatting-tour", "design-crit", "launch-war-room"]}

FORMATTING = [
    {"by": "lena", "at": (12, "09:30"), "pin": True,
     "text": "# Welcome to the formatting tour :wave:\n\nThis channel shows every kind of message Matty draws: "
             "headings, lists, tables, code, quotes, math, emoji, files and more. Each post is one example."},
    {"by": "lena", "at": (12, "09:32"),
     "text": "# Heading 1\n## Heading 2\n### Heading 3\n#### Heading 4\n##### Heading 5\n###### Heading 6"},
    {"by": "sam", "at": (12, "09:35"),
     "text": "Inline styles: **bold**, *italic*, ***bold italic***, ~~strikethrough~~, `inline code`, "
             "and a [link](https://example.com/docs). Autolinks work too: https://example.com/changelog"},
    {"by": "sam", "at": (12, "09:36"),
     "text": "Lists can nest:\n\n- Design\n  - Tokens\n  - Components\n    - Button\n    - Dialog\n- Engineering\n- Launch\n\n"
             "1. Write the brief\n2. Review it\n   1. With design\n   2. With engineering\n3. Ship it"},
    {"by": "aiko", "at": (12, "09:40"), "react": {"white_check_mark": ["maya", "theo"]},
     "text": "Task lists for checklists:\n\n- [x] Pricing page copy\n- [x] Screenshots for the store listing\n- [ ] Press kit\n- [ ] Launch email"},
    {"by": "ravi", "at": (12, "10:02"), "save": True,
     "text": "Tables, with alignment:\n\n| Plan | Seats | Price | Trial |\n| :--- | ---: | ---: | :---: |\n"
             "| Starter | 10 | $0 | — |\n| Team | 50 | $8 | 14 days |\n| Business | 500 | $14 | 30 days |\n| Enterprise | Unlimited | Talk to us | :white_check_mark: |"},
    {"by": "priya", "at": (12, "10:15"),
     "text": "Code blocks are highlighted by language:\n\n```rust\nuse std::collections::HashMap;\n\n/// Counts words, case-insensitively.\nfn count_words(text: &str) -> HashMap<String, usize> {\n"
             "    let mut counts = HashMap::new();\n    for word in text.split_whitespace() {\n        *counts.entry(word.to_lowercase()).or_insert(0) += 1;\n    }\n    counts\n}\n```"},
    {"by": "sam", "at": (12, "10:18"),
     "text": "```typescript\ninterface Launch {\n  name: string;\n  date: Date;\n  owners: string[];\n}\n\nexport function daysLeft(launch: Launch, now = new Date()): number {\n"
             "  const ms = launch.date.getTime() - now.getTime();\n  return Math.max(0, Math.ceil(ms / 86_400_000));\n}\n```"},
    {"by": "ravi", "at": (12, "10:20"),
     "text": "```sql\nSELECT plan, COUNT(*) AS signups, AVG(seats) AS avg_seats\nFROM accounts\nWHERE created_at >= now() - interval '30 days'\nGROUP BY plan\nORDER BY signups DESC;\n```"},
    {"by": "jonas", "at": (12, "10:25"),
     "text": "```python\nimport asyncio\n\nasync def probe(host: str) -> bool:\n    reader, writer = await asyncio.open_connection(host, 443)\n    writer.close()\n    await writer.wait_closed()\n    return True\n```\n\n```bash\n$ kubectl rollout status deploy/api -n prod\ndeployment \"api\" successfully rolled out\n```"},
    {"by": "theo", "at": (12, "10:31"),
     "text": "Diffs read well in review threads:\n\n```diff\n-const TIMEOUT = 5_000;\n+const TIMEOUT = 15_000; // slow networks on the trial plan\n export async function fetchPlans() {\n```\n\nAnd JSON:\n\n```json\n{\n  \"plan\": \"team\",\n  \"seats\": 50,\n  \"features\": [\"sso\", \"audit-log\"]\n}\n```"},
    {"by": "lena", "at": (12, "11:00"),
     "text": "> Design is not just what it looks like and feels like. Design is how it works.\n>\n> > Quotes can nest, too.\n\nBack to normal text."},
    {"by": "ravi", "at": (12, "11:20"),
     "text": "Math, inline like $e^{i\\pi} + 1 = 0$, or as a block:\n\n$$\n\\text{churn} = \\frac{\\sum_{m} \\text{cancelled}_m}{\\sum_{m} \\text{active}_m}\n$$"},
    {"by": "diego", "at": (12, "11:45"), "react": {"joy": ["sam", "lena", "maya"], "partying_face": ["aiko"]},
     "text": "Emoji, in a sentence :tada: :rocket: :sparkles: or on their own:\n\n:fire: :100:"},
    {"by": "aiko", "at": (12, "12:05"),
     "text": "Mentions highlight people and groups: @maya @theo, and channels like ~launch-v2. Use @here sparingly :sweat_smile:"},
    {"by": "lena", "at": (12, "13:10"), "files": ["mockup-home.png", "mockup-settings.png"],
     "text": "Images attach as a gallery. These are the latest home and settings mocks."},
    {"by": "ravi", "at": (12, "13:30"), "files": ["signups-chart.png"],
     "text": "A single image shows larger. Signups by week since the beta opened :chart_with_upwards_trend:"},
    {"by": "aiko", "at": (12, "14:00"), "files": ["launch-plan.pdf", "pricing.csv", "release-notes.md"],
     "text": "Other files show as cards you can download."},
    {"by": "lumen-ci", "at": (12, "14:12"),
     "text": "", "props": {"attachments": [{
         "fallback": "Build #1842 passed", "color": "#16a34a", "pretext": "Deploy to **staging** finished",
         "author_name": "Lumen CI", "title": "Build #1842 · main · 4m 12s", "title_link": "https://example.com/ci/1842",
         "text": "All 1,284 tests passed. Bundle size **-12 kB** :tada:",
         "fields": [{"title": "Commit", "value": "`a1b9c3e` Fix plan picker focus", "short": True},
                    {"title": "Author", "value": "@sam", "short": True}],
         "footer": "Lumen CI"}]}},
    {"by": "lumen-ci", "at": (12, "14:40"),
     "text": "", "props": {"attachments": [{
         "fallback": "Build #1843 failed", "color": "#dc2626", "title": "Build #1843 · launch/pricing · failed",
         "title_link": "https://example.com/ci/1843",
         "text": "```\nFAIL  src/pricing/total.test.ts\n  expected 1400, received 1399.99\n```",
         "footer": "Lumen CI"}]}},
    {"by": "sam", "at": (12, "15:00"),
     "text": "Horizontal rules split long posts\n\n---\n\nlike this. And a line\nbreak inside a paragraph."},
    {"by": "elif", "at": (12, "15:20"),
     "text": "Accents and other alphabets: café, naïve, Zürich, Ελληνικά, Кириллица."},
    {"by": "theo", "at": (12, "16:00"), "edit": "Edited messages say so. This one was fixed after a typo. :pencil2:",
     "text": "Edited messages say so. This one has a tpyo."},
    {"by": "sam", "at": (12, "16:30"),
     "text": "Threads keep side conversations out of the channel. Open the replies on this one :point_down:",
     "replies": [
         {"by": "lena", "at": 3, "text": "Replies show here, with their own composer."},
         {"by": "maya", "at": 5, "text": "And the channel shows how many replies there are and who joined in."},
         {"by": "theo", "at": 9, "text": "You can follow a thread to see new replies in **Threads**."},
     ]},
]

LAUNCH = [
    {"by": "aiko", "at": (18, "09:10"), "pin": True,
     "text": "Kicking off **Lumen 2.0** :rocket:\n\nLaunch is **five weeks out**. The plan, owners and milestones are in the channel header. "
             "Please read it before Thursday's sync and add questions as replies here.",
     "react": {"rocket": ["theo", "maya", "lena", "sam", "priya", "nora"], "eyes": ["jonas"]},
     "replies": [
         {"by": "theo", "at": 12, "text": "Engineering is on track for code freeze in four weeks. The one risk is the billing migration."},
         {"by": "nora", "at": 25, "text": "Marketing needs final screenshots three weeks from now for the store listings."},
         {"by": "maya", "at": 40, "text": "Design can hit that. I'll share the screenshot list tomorrow."},
         {"by": "aiko", "at": 55, "text": "Great, thanks all. I'll keep the board up to date."},
     ]},
    {"by": "maya", "at": (17, "10:05"), "save": True,
     "text": "Screenshot list for the store listings:\n\n1. Home with the new sidebar\n2. Thread view\n3. Search with filters\n4. Dark mode\n5. Settings\n\n@nora shout if a channel needs a different size.",
     "replies": [{"by": "nora", "at": 20, "text": "Perfect. 1280×800 for all of them works."}]},
    {"by": "priya", "at": (15, "14:20"),
     "text": "Billing migration dry run on staging: 41,203 accounts migrated in 6m 40s, zero mismatches :tada:",
     "react": {"tada": ["theo", "aiko", "maya", "marcus"], "rocket": ["jonas", "sam"]}},
    {"by": "aiko", "at": (14, "09:00"),
     "text": "Weekly status: :large_green_circle: Engineering, :large_green_circle: Design, :large_yellow_circle: Marketing (press kit late), :large_green_circle: Support."},
    {"by": "nora", "at": (13, "11:30"), "files": ["press-kit-cover.png"],
     "text": "Press kit cover draft. Feedback welcome before I send it to the agency.",
     "replies": [
         {"by": "lena", "at": 15, "text": "Love it. Could the logo get a bit more room at the top?"},
         {"by": "nora", "at": 32, "text": "Good call, updated."},
     ]},
    {"by": "theo", "at": (8, "17:45"),
     "text": "**Code freeze is in effect** :snowflake: Only launch blockers merge from now on; tag them `launch-blocker`.",
     "pin": True, "react": {"snowflake": ["priya", "sam", "elif", "jonas"]}},
    {"by": "aiko", "at": (6, "10:00"),
     "text": "Go/no-go is next Friday at 15:00. Each area brings a one-line status. I'll send the invite."},
    {"by": "sam", "at": (3, "16:10"),
     "text": "All launch blockers on the web client are closed. :white_check_mark: The last one was the plan picker losing focus.",
     "react": {"tada": ["theo", "maya", "aiko"]}},
    {"by": "marcus", "at": (1, "09:15"),
     "text": "Proud of this team. Whatever happens on launch day, this is the best release we've built. Thank you all :pray:",
     "react": {"heart": EVERYONE[:9]}},
]

DESIGN = [
    {"by": "lena", "at": (20, "10:00"), "pin": True,
     "text": "Design principles for 2.0, so we argue about the same things:\n\n1. **Calm by default.** Color means something.\n2. **Fast to scan.** Names, times and unread state first.\n3. **Keyboard first, mouse friendly.**\n4. **Respect the platform.**"},
    {"by": "maya", "at": (19, "14:30"), "files": ["mockup-home.png"],
     "text": "New sidebar exploration. Unread channels get weight, not color; mentions get the badge.",
     "replies": [
         {"by": "lena", "at": 10, "text": "This is much calmer. What happens with 40 unread channels?"},
         {"by": "maya", "at": 18, "text": "They sort to the top within their category if you turn that on. Otherwise it stays alphabetical."},
         {"by": "sam", "at": 30, "text": "Implementation-wise that's easy, the sort already exists."},
         {"by": "aiko", "at": 45, "text": "Let's ship it with sorting off by default and see what people ask for."},
     ], "react": {"heart_eyes": ["lena", "sam"], "+1": ["aiko"]}},
    {"by": "lena", "at": (16, "11:00"),
     "text": "Reminder: the spacing scale is 4px based. `4 8 12 16 24 32 48`. If you need 20, you probably need 24 :wink:"},
    {"by": "sam", "at": (14, "15:40"), "files": ["dark-mode.png"],
     "text": "Dark mode with the new tokens. Contrast checks pass at AA for all text.",
     "react": {"new_moon_with_face": ["maya", "lena", "elif"]}},
    {"by": "maya", "at": (11, "10:20"),
     "text": "Icon audit done: 214 icons in the app, 37 were duplicates or near-duplicates. Cleanup PR is up.",
     "replies": [{"by": "lena", "at": 25, "text": "37! Thank you for doing this."}]},
    {"by": "lena", "at": (7, "14:00"),
     "text": "Crit today at 14:00: settings and the new onboarding. Bring coffee :coffee:"},
    {"by": "maya", "at": (2, "11:15"), "files": ["mockup-settings.png"],
     "text": "Final settings layout. Grouped by what people try to do, not by where the code lives."},
]

ENGINEERING = [
    {"by": "theo", "at": (20, "09:40"), "pin": True,
     "text": "Engineering norms, short version:\n\n- PRs under 400 lines when you can\n- Every PR links an issue\n- CI must be green to merge\n- Review within one working day"},
    {"by": "priya", "at": (18, "13:00"),
     "text": "RFC: move background jobs from cron to a queue. Doc: https://example.com/rfc/017-job-queue",
     "replies": [
         {"by": "jonas", "at": 20, "text": "Strong yes. Cron on three hosts has bitten us twice this quarter."},
         {"by": "theo", "at": 35, "text": "What's the plan for jobs that must not run twice?"},
         {"by": "priya", "at": 50, "text": "Idempotency keys plus a lease. Section 4 of the doc."},
         {"by": "theo", "at": 70, "text": "Read it, makes sense. Approved from my side."},
     ]},
    {"by": "sam", "at": (16, "16:20"),
     "text": "PSA: the dev build is 40% faster to start since we dropped the old icon font. `npm run dev` should feel snappy now :zap:",
     "react": {"zap": ["theo", "elif", "maya"]}},
    {"by": "jonas", "at": (13, "10:10"),
     "text": "Postgres 16 upgrade on staging is done. Prod is scheduled for Saturday 06:00 UTC, expected downtime under 5 minutes."},
    {"by": "elif", "at": (10, "15:00"),
     "text": "Mobile 2.0 beta is in TestFlight and the Play internal track. Install links in the thread.",
     "replies": [{"by": "elif", "at": 1, "text": "iOS: https://example.com/beta/ios\nAndroid: https://example.com/beta/android"}]},
    {"by": "theo", "at": (5, "11:30"), "save": True,
     "text": "Reminder that load tests run tonight against staging. Expect alerts in ~incidents between 22:00 and 23:00."},
    {"by": "priya", "at": (2, "14:45"),
     "text": "```\np50  42ms\np95 180ms\np99 410ms\n```\nAPI latency after the cache change. p99 halved :tada:",
     "react": {"tada": ["theo", "jonas", "sam", "aiko"]}},
]

FRONTEND = [
    {"by": "sam", "at": (9, "10:00"),
     "text": "Should the plan picker remember the last plan someone looked at?",
     "replies": [
         {"by": "maya", "at": 6, "text": "Yes for returning visitors, no for the first visit. First visit should start on Team."},
         {"by": "sam", "at": 15, "text": "Makes sense. I'll keep it in localStorage."},
     ]},
    {"by": "elif", "at": (4, "13:20"), "text": "The shared components package is on 3.0 now. Breaking change: `Button size=\"xs\"` is gone."},
]

INCIDENTS = [
    {"by": "jonas", "at": (11, "22:14"), "pin": True,
     "text": ":rotating_light: **SEV-2: elevated API errors** in eu-west. Investigating.",
     "replies": [
         {"by": "jonas", "at": 4, "text": "Errors are from one bad node behind the load balancer. Draining it."},
         {"by": "priya", "at": 7, "text": "Seeing the same in the job queue logs, retries are piling up."},
         {"by": "jonas", "at": 12, "text": "Node drained, error rate back to baseline."},
         {"by": "jonas", "at": 30, "text": ":white_check_mark: Resolved. Post-mortem tomorrow."},
     ]},
    {"by": "jonas", "at": (10, "15:00"), "files": ["postmortem-api-errors.md"],
     "text": "Post-mortem for yesterday's SEV-2 is up. TL;DR: a kernel update left one node with a broken network driver; health checks didn't catch it. Action items inside."},
    {"by": "lumen-ci", "at": (5, "22:05"), "text": "",
     "props": {"attachments": [{"fallback": "Load test started", "color": "#ca8a04", "title": "Load test started · staging",
                                "text": "Ramping to 20k requests/second over 30 minutes.", "footer": "Lumen CI"}]}},
]

HELP_DESK = [
    {"by": "diego", "at": (6, "10:30"),
     "text": "Customer asks if they can move from Team to Business mid-cycle without losing their discount. Anyone know?",
     "replies": [
         {"by": "aiko", "at": 12, "text": "Yes, it's prorated and the discount carries over for the rest of the term."},
         {"by": "diego", "at": 20, "text": "Thank you! Adding that to the help center."},
     ]},
    {"by": "diego", "at": (2, "16:00"),
     "text": "Weekly support stats: 312 tickets, median first reply **14 minutes**, CSAT 96% :tada:",
     "react": {"tada": ["aiko", "marcus", "maya"]}},
]

TOWN_SQUARE = [
    {"by": "marcus", "at": (20, "09:00"),
     "text": "Good morning everyone! All-hands is Friday at 16:00. Agenda: launch, hiring, and the offsite :sunrise:"},
    {"by": "aiko", "at": (15, "12:00"), "text": "Welcome @elif to the team :wave: Elif joins us as a mobile engineer!",
     "react": {"wave": EVERYONE[:10]}, "replies": [{"by": "elif", "at": 30, "text": "Thank you, happy to be here!"}]},
    {"by": "nora", "at": (9, "09:30"), "text": "Our 2.0 teaser got 18k views in a day. Thanks for sharing it around! :mega:"},
    {"by": "marcus", "at": (4, "17:00"), "text": "Offsite is confirmed: Lisbon, in the spring. Details from @aiko soon :airplane:",
     "react": {"airplane": ["maya", "sam", "lena", "theo"], "heart": ["nora", "diego"]}},
]

OFF_TOPIC = [
    {"by": "sam", "at": (17, "12:30"), "text": "Lunch spot recommendation: the new ramen place on 5th. The spicy miso is excellent :ramen:"},
    {"by": "lena", "at": (13, "08:50"), "files": ["plant.png"], "text": "My monstera finally grew a new leaf :herb:",
     "react": {"seedling": ["maya", "diego", "nora"]}},
    {"by": "jonas", "at": (8, "18:20"), "text": "Anyone up for climbing on Thursday?",
     "replies": [{"by": "elif", "at": 10, "text": "Yes! 19:00?"}, {"by": "jonas", "at": 14, "text": "19:00 it is."}]},
    {"by": "diego", "at": (3, "12:10"), "text": "What's everyone listening to this week? I'm stuck on old jazz records :musical_note:"},
]

DESIGN_CRIT = [
    {"by": "lena", "at": (7, "14:05"), "files": ["onboarding.png"],
     "text": "Onboarding v3. Be harsh.",
     "replies": [
         {"by": "maya", "at": 8, "text": "Step 2 asks for too much. Could the team name wait until they invite someone?"},
         {"by": "sam", "at": 12, "text": "Agree with Maya. Also the progress dots are hard to see in dark mode."},
         {"by": "aiko", "at": 20, "text": "Data says 30% drop at step 2, so yes, let's cut it."},
     ]},
]

WAR_ROOM = [
    {"by": "aiko", "at": (2, "15:00"), "pin": True,
     "text": "Go/no-go checklist:\n\n- [x] Billing migration verified\n- [x] Status page ready\n- [x] Support macros updated\n- [ ] Final store screenshots\n- [ ] Launch email scheduled"},
    {"by": "marcus", "at": (2, "15:20"), "text": "We're a go from my side. :green_circle:"},
]

STANDUP_LINES = {
    "yesterday": [
        "Finished the plan picker focus fix", "Reviewed three PRs", "Paired with Priya on the job queue",
        "Wrote the migration runbook", "Shipped the new empty states", "Cleaned up the icon set",
        "Answered support escalations", "Fixed the flaky search test", "Drafted the launch email",
        "Updated the pricing table", "Tuned the API cache", "Tested the beta on Android",
    ],
    "today": [
        "Store screenshots", "Load test prep", "Billing edge cases", "Onboarding copy",
        "Review backlog", "Dark mode polish", "Help center articles", "Release notes",
        "Metrics dashboard", "Push notification retries", "Accessibility pass", "Docs for the API changes",
    ],
}
STANDUP_PEOPLE = ["theo", "priya", "sam", "lena", "jonas", "aiko", "elif"]

COMMUNITY = {
    "town-square": [
        {"by": "nora", "at": (19, "10:00"), "pin": True,
         "text": "Welcome to the Lumen Community! :wave: Introduce yourself in a reply: what do you use Lumen for?",
         "replies": [
             {"by": "ravi", "at": 30, "text": "Ravi here, I build dashboards on the Lumen API."},
             {"by": "elif", "at": 75, "text": "Elif, mostly mobile things."},
         ]},
    ],
    "show-and-tell": [
        {"by": "ravi", "at": (9, "16:00"), "files": ["signups-chart.png"],
         "text": "Made a little dashboard with the public API. Code is in the thread if anyone wants it."},
    ],
    "plugins": [
        {"by": "priya", "at": (6, "11:00"),
         "text": "Plugin API docs got an overhaul: https://example.com/developers/plugins :books:"},
    ],
}

# Direct messages with the demo user: other person -> posts.
DIRECT = {
    "lena": [
        {"by": "lena", "at": (5, "09:40"), "text": "Hey! Do you have ten minutes for the onboarding flow today?"},
        {"by": "maya", "at": (5, "09:52"), "text": "Sure, after lunch? 13:30?"},
        {"by": "lena", "at": (5, "09:53"), "text": "Perfect :+1:"},
        {"by": "lena", "at": (1, "17:30"), "text": "Thanks for today, the new flow is so much better."},
    ],
    "theo": [
        {"by": "theo", "at": (4, "11:00"), "text": "Can you sanity check the error copy in the billing PR? Link: https://example.com/pr/2291"},
        {"by": "maya", "at": (4, "11:40"), "text": "Left a few suggestions. Mostly making the next step clear."},
    ],
    "priya": [
        {"by": "priya", "at": (7, "15:15"), "text": "Is the empty state for the job queue view final? I'd like to wire it up."},
        {"by": "maya", "at": (7, "15:30"), "text": "Yes, it's in the Figma file under Queue / Empty."},
    ],
    "aiko": [
        {"by": "aiko", "at": (3, "10:00"), "text": "Quick one: are we still on track for the screenshots?"},
        {"by": "maya", "at": (3, "10:05"), "text": "Yes, four of five are done. Dark mode is last."},
    ],
}
SELF_NOTES = [
    {"by": "maya", "at": (6, "08:30"), "text": "Notes to self:\n\n- [ ] Export icons at 1x and 2x\n- [x] Send Nora the screenshot sizes\n- [ ] Book the user interviews"},
]
GROUP = (["lena", "sam"], [
    {"by": "sam", "at": (2, "12:00"), "text": "Lunch? The ramen place again?"},
    {"by": "lena", "at": (2, "12:02"), "text": "Always :ramen:"},
    {"by": "maya", "at": (2, "12:03"), "text": "Count me in!"},
])

# Posts that arrive after the demo user last read: unread channels and
# mentions in the sidebar. (team, channel) or ("dm", username).
UNREAD = [
    (("lumen", "launch-v2"), {"by": "aiko", "text": "@maya are the final store screenshots ready? Nora wants to send them today."}),
    (("lumen", "launch-v2"), {"by": "nora", "text": "Store listing is staged, waiting on screenshots only :camera:"}),
    (("lumen", "design"), {"by": "lena", "text": "Crit notes from today are in the doc. Nice work on the settings page, everyone."}),
    (("lumen", "engineering"), {"by": "jonas", "text": "Prod Postgres upgrade is done. 3m 12s of downtime :tada:"}),
    (("lumen", "incidents"), {"by": "lumen-ci", "text": ":white_check_mark: Load test finished. Peak 21.4k req/s, no errors."}),
    (("dm", "lena"), {"by": "lena", "text": "Saw the final screenshots, they look great! :sparkles:"}),
]
