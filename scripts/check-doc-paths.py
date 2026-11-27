#!/usr/bin/env python3
"""Check local links and repository path literals in the active project guides."""

import re
import sys
from pathlib import Path
from urllib.parse import unquote, urlsplit


ROOT = Path(__file__).resolve().parents[1]
# These top-level docs describe the tree at an earlier revision (see docs/README.md).
HISTORICAL = {
    "go-i2p-leaseset-analysis.md",
    "handoff-brief.md",
    "i2p-datagrams-evaluation.md",
    "i2p-transport-evaluation.md",
    "relay-independence.md",
    "today-2026-10-28.md",
}
LINK = re.compile(r'(?:\]\(\s*|^ {0,3}\[[^\]]+\]:\s*)(?:<([^>]+)>|([^\s)]+))')
HTML_LINK = re.compile(r'''\b(?:src|href)\s*=\s*["']([^"']+)["']''', re.I)
CODE_SPAN = re.compile(r"`([^`]+)`")
REPO_DIRS = (".github", "agent", "android-router", "bot-sdk", "browser-router", "core",
             "dht", "docs", "i2p-embed", "libcore", "scripts", "site", "tests",
             "third_party", "tools", "ui")
LITERAL = re.compile(r"([\w./{},*+-]+)(?::\d+(?:[-–]\d+|\+)?)?")
GENERATED = {"ui/browser-dist", "ui/dist"}


def docs():
    for name in ("README.md", "README.en.md", "TASKS.md", ".github/workflows/README.md"):
        yield ROOT / name
    yield from (doc for doc in sorted((ROOT / "docs").glob("*.md")) if doc.name not in HISTORICAL)


def expand(path):
    """Expand a documented brace list such as src/locales/{en,ru}.ts."""
    group = re.search(r"\{([^{}]+)\}", path)
    if group:
        for option in group[1].split(","):
            yield from expand(path[:group.start()] + option + path[group.end():])
    else:
        yield path


def code_paths(doc, snippet):
    # Package-relative commands (e.g. node tests/... inside ui) have their own
    # working directory. Shell/Python invocations below use repository scripts.
    words = snippet.split()
    if words and words[0] in {"python3", "bash", "sh", "busybox"}:
        literals = [word for word in words if word.endswith((".py", ".sh"))]
    else:
        literals = words[:1]
    for literal in literals:
        matched = LITERAL.fullmatch(literal)
        if not matched:
            continue
        path = matched[1].rstrip("/")
        if path.startswith(("./", "../")):
            base = doc.parent
        elif path.partition("/")[0] in REPO_DIRS and "/" in literal:
            base = ROOT
        elif path.startswith("/tmp/") and path.endswith(".md"):
            base = ROOT  # A temporary review file cannot serve as repository evidence.
        else:
            continue
        if path not in GENERATED:
            for item in expand(path):
                yield base, item


def references(doc, line):
    for destination in [a or b for a, b in LINK.findall(line)] + HTML_LINK.findall(line):
        url = urlsplit(destination)
        if url.path and not (url.scheme or url.netloc):
            yield doc.parent, unquote(url.path)
    for snippet in CODE_SPAN.findall(line):
        yield from code_paths(doc, snippet)


def main():
    checked = 0
    errors = 0
    for doc in docs():
        fence = None
        for number, line in enumerate(doc.read_text(encoding="utf-8").splitlines(), 1):
            marker = re.match(r"^\s*(`{3,}|~{3,})", line)
            if marker:
                if fence is None:
                    fence = marker[1]
                elif marker[1].startswith(fence):
                    fence = None
                continue
            refs = code_paths(doc, line) if fence else references(doc, line)
            for base, path in refs:
                checked += 1
                target = base / path
                if target.resolve().is_relative_to(ROOT):
                    if "*" in path and any(base.glob(path)):
                        continue
                    if target.exists():
                        continue
                errors += 1
                print(f"{doc.relative_to(ROOT)}:{number}: missing repository path {path}", file=sys.stderr)
    print(f"Checked {checked} documentation references, {errors} missing.")
    return int(errors != 0)


if __name__ == "__main__":
    sys.exit(main())
