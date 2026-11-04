#!/usr/bin/env python3
"""Lists gpuicn Buttons in crates/matty-app/src that set neither `label` nor
`aria_label` (PLAN §8: every control has an accessible name). gpuicn's
`Button::child` adds content without naming the button; use `label` for
plain text and `aria_label` for icons or mixed content. Exits non-zero when
any are found."""
import glob
import re
import sys
from pathlib import Path

SRC = Path(__file__).resolve().parent.parent / "crates/matty-app/src"


def chains(lines, start):
    """The builder chain that starts on `start`: following lines that are
    blank, indented deeper, or continue with a `.method`."""
    first = lines[start]
    indent = len(first) - len(first.lstrip())
    out = [first]
    depth = first.count("(") - first.count(")")
    for line in lines[start + 1 : start + 60]:
        if not line.strip() or line.lstrip().startswith("//"):
            continue
        deeper = len(line) - len(line.lstrip()) > indent
        # A `Button::new(` whose id spans lines closes its parentheses first.
        if depth <= 0 and not deeper and not line.lstrip().startswith("."):
            break
        out.append(line)
        depth += line.count("(") - line.count(")")
    return "\n".join(out)


def main():
    missing = []
    for path in sorted(glob.glob(str(SRC / "**" / "*.rs"), recursive=True)):
        lines = Path(path).read_text().split("\n")
        for index, line in enumerate(lines):
            if "Button::new(" not in line:
                continue
            chain = chains(lines, index)
            if ".aria_label(" in chain or ".label(" in chain:
                continue
            # A helper that returns the Button for the caller to name.
            if re.search(r"\|.*\|\s*\{?\s*$", lines[index - 1] if index else ""):
                continue
            missing.append(f"{Path(path).name}:{index + 1}: {line.strip()}")
    for entry in missing:
        print(entry)
    print(f"{len(missing)} unnamed")
    return 1 if missing else 0


if __name__ == "__main__":
    sys.exit(main())
