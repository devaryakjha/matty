#!/usr/bin/env python3
"""Imports Markdown test cases from the Mattermost webapp into
crates/matty-markdown/fixtures/webapp.json.

    git clone --depth 1 https://github.com/mattermost/mattermost.git /tmp/mm
    scripts/import-markdown-fixtures.py /tmp/mm

It reads the webapp's text formatting and Markdown tests, keeps each
`expect(format(...)).toBe(...)` whose input and expected output are string
literals, and rewrites the expected HTML into the canonical form that
`matty_markdown::to_html` prints: Matty's `matty://` links for mentions,
channels, emoji and hashtags, and no classes or wrapper spans. Cases that
depend on options Matty does not have (search highlighting, mention keys,
team URLs) are skipped and counted.

The webapp is Apache-2.0 licensed (see NOTICE). Only the Python standard
library is used.
"""

from __future__ import annotations

import argparse
import html
import html.parser
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUTPUT = ROOT / "crates" / "matty-markdown" / "fixtures" / "webapp.json"
SOURCES = [
    "webapp/channels/src/utils/text_formatting.test.ts",
    "webapp/channels/src/utils/text_formatting_at_mentions.test.ts",
    "webapp/channels/src/utils/text_formatting_channel_links.test.ts",
    "webapp/channels/src/utils/text_formatting_email.test.ts",
    "webapp/channels/src/utils/text_formatting_hashtags.test.ts",
    "webapp/channels/src/utils/text_formatting_imgs.test.ts",
    "webapp/channels/src/utils/text_formatting_links.test.ts",
    "webapp/channels/src/utils/markdown/index.test.ts",
    "webapp/channels/src/utils/markdown/renderer.test.tsx",
]
CALLS = re.compile(r"\b(?:Markdown\.format|TextFormatting\.formatText|format)\(")
# Options that change nothing Matty renders.
NEUTRAL_OPTIONS = {"", "{}", "{atMentions: true}", "{markdown: true}"}


class Lexer:
    """Reads JavaScript string expressions: literals joined with `+`."""

    def __init__(self, text: str, at: int):
        self.text = text
        self.at = at

    def skip(self) -> None:
        while self.at < len(self.text):
            if self.text[self.at].isspace():
                self.at += 1
            elif self.text.startswith("//", self.at):
                self.at = self.text.find("\n", self.at)
                if self.at < 0:
                    self.at = len(self.text)
            else:
                return

    def literal(self) -> str | None:
        self.skip()
        if self.at >= len(self.text):
            return None
        quote = self.text[self.at]
        if quote not in "'\"`":
            return None
        out = []
        i = self.at + 1
        while i < len(self.text):
            c = self.text[i]
            if c == "\\":
                nxt = self.text[i + 1]
                escapes = {"n": "\n", "t": "\t", "r": "\r", "0": "\0"}
                if nxt == "u":
                    out.append(chr(int(self.text[i + 2:i + 6], 16)))
                    i += 6
                    continue
                if nxt == "\n":
                    i += 2
                    continue
                out.append(escapes.get(nxt, nxt))
                i += 2
                continue
            if quote == "`" and self.text.startswith("${", i):
                return None
            if c == quote:
                self.at = i + 1
                return "".join(out)
            out.append(c)
            i += 1
        return None

    def expression(self) -> str | None:
        """A literal, or literals joined with `+`; None for anything else."""
        parts = []
        while True:
            part = self.literal()
            if part is None:
                return None
            parts.append(part)
            self.skip()
            if self.text.startswith("+", self.at):
                self.at += 1
                continue
            return "".join(parts)

    def balanced(self) -> str:
        """Raw text up to the `)` or `,` that ends this argument."""
        depth = 0
        start = self.at
        while self.at < len(self.text):
            c = self.text[self.at]
            if c in "([{":
                depth += 1
            elif c in ")]}":
                if depth == 0:
                    break
                depth -= 1
            elif c == "," and depth == 0:
                break
            elif c in "'\"`":
                if self.literal() is None:
                    self.at += 1
                continue
            self.at += 1
        return self.text[start:self.at].strip()


def string_constants(text: str) -> dict[str, list[tuple[int, str]]]:
    """`const name = <string expression>;` definitions, by name."""
    found: dict[str, list[tuple[int, str]]] = {}
    for match in re.finditer(r"\bconst (\w+) =", text):
        lexer = Lexer(text, match.end())
        value = lexer.expression()
        if value is not None:
            found.setdefault(match.group(1), []).append((match.start(), value))
    return found


def argument(lexer: Lexer, constants, at: int) -> str | None:
    """A string argument: a literal expression or a constant defined above."""
    start = lexer.at
    value = lexer.expression()
    if value is not None:
        return value
    lexer.at = start
    lexer.skip()
    name = re.match(r"\w+", lexer.text[lexer.at:])
    if not name or name.group(0) not in constants:
        return None
    defined = [v for (pos, v) in constants[name.group(0)] if pos < at]
    if not defined:
        return None
    lexer.at += len(name.group(0))
    return defined[-1]


def cases(path: Path, relative: str):
    text = path.read_text()
    constants = string_constants(text)
    for match in re.finditer(r"\bexpect\(\s*", text):
        lexer = Lexer(text, match.end())
        call = CALLS.match(text, lexer.at)
        if not call:
            continue
        line = text.count("\n", 0, match.start()) + 1
        lexer.at = call.end()
        source = argument(lexer, constants, match.start())
        if source is None:
            yield None, "the input is not a string literal"
            continue
        lexer.skip()
        options = ""
        if text.startswith(",", lexer.at):
            lexer.at += 1
            lexer.skip()
            options = lexer.balanced()
        # Skip the rest of the call, `.trim()` and the like, to the matcher.
        matcher = re.compile(r"\)\s*\.\s*(toBe|toEqual|toContain)\(")
        found = matcher.search(text, lexer.at)
        if not found or "expect(" in text[lexer.at:found.start()]:
            yield None, "no matcher"
            continue
        if found.group(1) == "toContain":
            yield None, "a partial match (toContain)"
            continue
        lexer.at = found.end()
        expected = argument(lexer, constants, match.start())
        if expected is None:
            yield None, "the expected output is not a string literal"
            continue
        if options not in NEUTRAL_OPTIONS:
            yield None, f"options Matty does not have: {options[:40]}"
            continue
        yield {
            "id": f"{Path(relative).stem}:{line}",
            "input": source,
            "webapp": expected,
        }, None


SPECIAL = {"here", "channel", "all"}
KEEP = {
    "p", "h1", "h2", "h3", "h4", "h5", "h6", "pre", "code", "blockquote",
    "ul", "ol", "li", "table", "tr", "th", "td", "em", "strong", "del", "a",
    "img", "br", "hr",
}
ATTRIBUTES = {"a": ["href", "title"], "img": ["src", "alt", "title"], "ol": ["start"],
              "th": ["align"], "td": ["align"]}


def escape(text: str) -> str:
    return (text.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")
            .replace('"', "&quot;"))


class Canonical(html.parser.HTMLParser):
    """The webapp's HTML in `to_html`'s form. Emoji names it shows are
    collected, so the test's resolver knows them."""

    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.out: list[str] = []
        self.closers: list[str] = []
        self.emoji: list[str] = []
        self.skip_depth = 0
        self.code_lang: str | None = None

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if self.skip_depth:
            if tag not in ("br", "img", "hr"):
                self.skip_depth += 1
            return
        if "data-mention" in attrs:
            name = attrs["data-mention"].lower()
            kind = "special" if name in SPECIAL else "user"
            self.out.append(f'<a href="matty://{kind}/{escape(name)}">')
            self.closers.append("</a>")
            return
        if "data-channel-mention" in attrs:
            name = attrs["data-channel-mention"].lower()
            self.out.append(f'<a href="matty://channel/{escape(name)}">~{escape(name)}</a>')
            self.closers.append("")
            self.skip_depth = 1
            return
        if "data-hashtag" in attrs:
            tag_name = attrs["data-hashtag"].lstrip("#")
            self.out.append(f'<a href="matty://hashtag/{escape(tag_name)}">')
            self.closers.append("</a>")
            return
        if "data-emoticon" in attrs:
            name = attrs["data-emoticon"]
            self.emoji.append(name)
            self.out.append(f'<a href="matty://emoji/{escape(name)}">:{escape(name)}:</a>')
            self.closers.append("")
            self.skip_depth = 1
            return
        if tag == "div" and "data-codeblock-code" in attrs:
            lang = attrs.get("data-codeblock-language") or ""
            code = attrs["data-codeblock-code"]
            head = f'<pre lang="{escape(lang)}">' if lang else "<pre>"
            self.out.append(f"{head}{escape(code)}\n</pre>")
            self.closers.append("")
            self.skip_depth = 1
            return
        if tag == "div" and "data-latex" in attrs:
            self.out.append(f'<pre lang="latex">{escape(attrs["data-latex"])}</pre>')
            self.closers.append("")
            self.skip_depth = 1
            return
        if tag not in KEEP:
            self.closers.append("")
            return
        if tag == "ol" and attrs.get("start") == "1":
            attrs.pop("start")
        kept = "".join(f' {name}="{escape(attrs[name])}"'
                       for name in ATTRIBUTES.get(tag, []) if attrs.get(name))
        self.out.append(f"<{tag}{kept}>")
        if tag not in ("br", "img", "hr"):
            self.closers.append(f"</{tag}>")

    def handle_startendtag(self, tag, attrs):
        self.handle_starttag(tag, attrs)

    def handle_endtag(self, tag):
        if self.skip_depth:
            self.skip_depth -= 1
            if self.skip_depth:
                return
        if self.closers:
            self.out.append(self.closers.pop())

    def handle_data(self, data):
        if not self.skip_depth:
            self.out.append(escape(data))


def canonical(webapp: str) -> tuple[str, list[str]]:
    parser = Canonical()
    parser.feed(webapp)
    parser.close()
    return "".join(parser.out), sorted(set(parser.emoji))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("checkout", type=Path, help="a clone of mattermost/mattermost")
    args = parser.parse_args()
    commit = subprocess.run(["git", "-C", str(args.checkout), "rev-parse", "HEAD"],
                            capture_output=True, text=True, check=True).stdout.strip()
    found, skipped = [], {}
    for relative in SOURCES:
        path = args.checkout / relative
        if not path.exists():
            sys.exit(f"missing {relative}: is this a mattermost/mattermost checkout?")
        for case, reason in cases(path, relative):
            if case is None:
                skipped[reason] = skipped.get(reason, 0) + 1
                continue
            case["expected"], case["emoji"] = canonical(case["webapp"])
            found.append(case)
    OUTPUT.parent.mkdir(parents=True, exist_ok=True)
    OUTPUT.write_text(json.dumps({
        "source": "https://github.com/mattermost/mattermost",
        "commit": commit,
        "license": "Apache-2.0 (webapp/); see NOTICE",
        "cases": found,
    }, indent=2, ensure_ascii=False) + "\n")
    print(f"wrote {len(found)} cases to {OUTPUT.relative_to(ROOT)}")
    for reason, count in sorted(skipped.items(), key=lambda item: -item[1]):
        print(f"  skipped {count}: {reason}")


if __name__ == "__main__":
    main()
