#!/usr/bin/env python3
"""Checks crates/matty-app/assets/theme.css against PLAN §8: 4.5:1 for text
tokens on the surfaces they sit on, 3:1 for presence and chart colors.
Exits non-zero on a failure. Hairline borders are decorative and not checked."""
import re
import sys
from pathlib import Path

CSS = Path(__file__).resolve().parent.parent / "crates/matty-app/assets/theme.css"
TEXT = 4.5
GRAPHIC = 3.0
PAIRS = [
    ("foreground", "background", TEXT),
    ("foreground", "accent", TEXT),
    ("foreground", "secondary", TEXT),
    ("foreground", "muted", TEXT),
    ("card-foreground", "card", TEXT),
    ("popover-foreground", "popover", TEXT),
    ("muted-foreground", "background", TEXT),
    ("muted-foreground", "card", TEXT),
    ("muted-foreground", "muted", TEXT),
    ("muted-foreground", "sidebar", TEXT),
    ("muted-foreground", "sidebar-accent", TEXT),
    ("sidebar-foreground", "sidebar", TEXT),
    ("sidebar-foreground", "sidebar-accent", TEXT),
    ("primary-foreground", "primary", TEXT),
    ("primary", "background", TEXT),
    ("primary", "card", TEXT),
    ("primary", "sidebar", TEXT),
    ("destructive", "background", TEXT),
    ("destructive", "card", TEXT),
    ("destructive", "sidebar", TEXT),
    ("ring", "background", GRAPHIC),
    ("ring", "sidebar", GRAPHIC),
    ("chart-2", "background", GRAPHIC),
    ("chart-2", "sidebar", GRAPHIC),
    ("chart-3", "background", GRAPHIC),
    ("chart-3", "sidebar", GRAPHIC),
    ("chart-4", "background", GRAPHIC),
    ("chart-4", "sidebar", GRAPHIC),
]


def block(css, selector):
    body = re.search(selector + r"\s*\{([^}]*)\}", css).group(1)
    return dict(re.findall(r"--([a-z0-9-]+):\s*(#[0-9a-fA-F]{6})", body))


def luminance(hex_color):
    channels = [int(hex_color[i : i + 2], 16) / 255 for i in (1, 3, 5)]
    linear = [c / 12.92 if c <= 0.03928 else ((c + 0.055) / 1.055) ** 2.4 for c in channels]
    return 0.2126 * linear[0] + 0.7152 * linear[1] + 0.0722 * linear[2]


def contrast(a, b):
    la, lb = luminance(a), luminance(b)
    hi, lo = max(la, lb), min(la, lb)
    return (hi + 0.05) / (lo + 0.05)


def main():
    css = CSS.read_text()
    light = block(css, ":root")
    dark = dict(light)
    dark.update(block(css, r"\.dark"))
    failed = False
    for name, tokens in (("light", light), ("dark", dark)):
        for fg, bg, need in PAIRS:
            ratio = contrast(tokens[fg], tokens[bg])
            mark = "ok  " if ratio >= need else "FAIL"
            if ratio < need:
                failed = True
            print(f"{mark} {name:5} {fg:18} on {bg:14} {ratio:5.2f} (need {need})")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
