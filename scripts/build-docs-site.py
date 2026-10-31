#!/usr/bin/env python3
"""Build the documentation site from every Markdown file in the repository.

The repository keeps each document next to the code it describes. This script
collects them into a single staging tree (``.docs-build/``) without moving the
sources, generates a categorized landing page, rewrites cross-document links,
and renders the result into ``site/`` with MkDocs. GitHub Pages publishes
``site/`` through ``.github/workflows/docs.yml``.

Usage:
    python3 scripts/build-docs-site.py            # stage + build site/
    python3 scripts/build-docs-site.py --stage-only
"""

from __future__ import annotations

import argparse
import os
import posixpath
import re
import shutil
import subprocess
import sys
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
STAGING = ROOT / ".docs-build"
OUTPUT = ROOT / "site"

# Build outputs and tool caches never contain documents.
EXCLUDE_DIRS = {
    ".git",
    ".docs-build",
    ".temp",
    "__archive__",
    "__sandbox__",
    "__ai-session-history__",
    "mutants.out",
    "mutants.out.old",
    "node_modules",
    "site",
    "target",
}

# Files that are copied next to the rendered pages. Everything else that is
# linked (source files, scripts, JSON evidence) points at GitHub instead.
ASSET_SUFFIXES = {
    ".gif",
    ".jpeg",
    ".jpg",
    ".pdf",
    ".png",
    ".svg",
    ".webp",
}

CATEGORIES = [
    "Overview",
    "Specifications & design",
    "Architecture Decision Records",
    "Measurement & gate reports",
    "Benchmark run reports",
    "Benchmarks",
    "Applications",
    "Crates",
    "Reference (historical)",
    "Other",
]

LINK_RE = re.compile(r"(!?\[)([^\]]*)(\]\()([^)\s]+)((?:\s+\"[^\"]*\")?\))")
TITLE_RE = re.compile(r"^#\s+(.+?)\s*$", re.MULTILINE)
SCHEME_RE = re.compile(r"^[a-zA-Z][a-zA-Z0-9+.-]*:")


def is_excluded(rel: Path) -> bool:
    return any(part in EXCLUDE_DIRS for part in rel.parts)


def collect_markdown() -> list[Path]:
    return sorted(
        rel
        for path in ROOT.rglob("*.md")
        if not (rel := path.relative_to(ROOT)) or not is_excluded(rel)
    )


def staged_path(rel: Path) -> Path:
    """Mirror the repository layout; MkDocs needs ``index.md`` for section roots."""
    parts = list(rel.parts)
    if parts[-1] == "README.md":
        parts[-1] = "index.md"
    return Path(*parts)


def document_title(rel: Path) -> str:
    text = (ROOT / rel).read_text(encoding="utf-8", errors="replace")
    match = TITLE_RE.search(text)
    if match:
        return match.group(1).strip().strip("#").strip()
    return rel.stem.replace("-", " ").replace("_", " ").strip().capitalize()


def category_for(rel: Path) -> str:
    parts = rel.parts
    top = parts[0]
    if top == "docs":
        if rel == Path("docs/README.md"):
            return "Overview"
        if len(parts) > 2 and parts[1] == "adr":
            return "Architecture Decision Records"
        if len(parts) > 2 and parts[1] == "reference":
            return "Reference (historical)"
        if re.match(r"^m\d", rel.name):
            return "Measurement & gate reports"
        return "Specifications & design"
    if top == "benchmarks":
        if len(parts) > 2 and parts[1] == "runs":
            return "Benchmark run reports"
        return "Benchmarks"
    if top == "apps":
        return "Applications"
    if top == "crates":
        return "Crates"
    return "Other"


def sort_key(rel: Path) -> list[tuple[int, object]]:
    return [
        (0, int(token)) if token.isdigit() else (1, token.lower())
        for token in re.split(r"(\d+)", rel.as_posix())
    ]


def repository_url() -> str | None:
    explicit = os.environ.get("DOCS_REPO_URL")
    if explicit:
        return explicit.rstrip("/")
    slug = os.environ.get("GITHUB_REPOSITORY")
    if slug:
        return f"https://github.com/{slug}"
    try:
        remote = subprocess.check_output(
            ["git", "-C", str(ROOT), "config", "--get", "remote.origin.url"],
            text=True,
            stderr=subprocess.DEVNULL,
        ).strip()
    except (OSError, subprocess.CalledProcessError):
        return None
    if not remote:
        return None
    if remote.startswith("git@") and ":" in remote:
        host, path = remote[4:].split(":", 1)
        remote = f"https://{host}/{path}"
    elif remote.startswith("ssh://"):
        remote = "https://" + remote[len("ssh://") :].split("@", 1)[-1]
    remote = re.sub(r"\.git$", "", remote)
    return remote.rstrip("/")


def branch_name() -> str:
    for variable in ("DOCS_BRANCH", "GITHUB_REF_NAME"):
        value = os.environ.get(variable)
        if value:
            return value
    try:
        return subprocess.check_output(
            ["git", "-C", str(ROOT), "symbolic-ref", "--short", "HEAD"],
            text=True,
            stderr=subprocess.DEVNULL,
        ).strip() or "main"
    except (OSError, subprocess.CalledProcessError):
        return "main"


def relative_link(from_staged: Path, to_staged: Path) -> str:
    return posixpath.relpath(to_staged.as_posix(), start=from_staged.parent.as_posix())


def rewrite_links(
    text: str,
    rel: Path,
    staged: Path,
    staged_by_source: dict[Path, Path],
    assets: set[Path],
    repo_url: str | None,
    branch: str,
) -> str:
    def replace(match: re.Match[str]) -> str:
        opening, label, separator, target, suffix = match.groups()
        if not target or target.startswith(("#", "//")) or SCHEME_RE.match(target):
            return match.group(0)
        path, _, fragment = target.partition("#")
        if not path:
            return match.group(0)

        resolved = Path(posixpath.normpath(posixpath.join(rel.parent.as_posix(), path)))
        # `join` discards the base for an absolute target, and `normpath` keeps
        # the leading "/", so an absolute path survived the `..` check and
        # reached the asset copy (and the staging tree) as an escape.
        if (
            resolved.is_absolute()
            or resolved.as_posix().startswith("..")
            or resolved == Path(".")
        ):
            return match.group(0)

        anchor = f"#{fragment}" if fragment else ""
        if resolved in staged_by_source:
            new = relative_link(staged, staged_by_source[resolved])
        elif (ROOT / resolved).is_dir():
            if not repo_url:
                return match.group(0)
            new = f"{repo_url}/tree/{branch}/{resolved.as_posix()}"
        elif (ROOT / resolved).is_file():
            if (ROOT / resolved).suffix.lower() in ASSET_SUFFIXES:
                # Resolve symlinks too: an asset pointing outside the checkout
                # would otherwise be copied into the published site.
                try:
                    contained = (ROOT / resolved).resolve().is_relative_to(ROOT.resolve())
                except OSError:
                    contained = False
                if not contained:
                    return match.group(0)
                assets.add(resolved)
                new = relative_link(staged, resolved)
            elif repo_url:
                new = f"{repo_url}/blob/{branch}/{resolved.as_posix()}"
            else:
                return match.group(0)
        else:
            return match.group(0)

        # A label that repeats the source path (for example
        # `apps/launcher/README.md`) should show the path it now points at.
        plain = label.strip("`")
        if plain == path:
            label = f"`{new}`" if plain != label else new
        return f"{opening}{label}{separator}{new}{anchor}{suffix}"

    return LINK_RE.sub(replace, text)


def landing_page(staged_by_source: dict[Path, Path]) -> str:
    grouped: dict[str, list[Path]] = defaultdict(list)
    for source in staged_by_source:
        grouped[category_for(source)].append(source)

    lines = [
        "# Voxel Engine documentation",
        "",
        "Every document in the repository, rendered as one site. The landing page",
        "is generated by `scripts/build-docs-site.py`; the sources stay next to the",
        "code they describe.",
        "",
    ]
    repo_url = repository_url()
    if repo_url:
        lines += [f"Source: [{repo_url.removeprefix('https://')}]({repo_url})", ""]

    for category in CATEGORIES:
        documents = grouped.get(category)
        if not documents:
            continue
        lines += [f"## {category}", "", "| Document | Source |", "|---|---|"]
        for source in sorted(documents, key=sort_key):
            target = staged_by_source[source].as_posix()
            title = document_title(source).replace("|", "\\|")
            lines.append(f"| [{title}]({target}) | `{source.as_posix()}` |")
        lines.append("")
    return "\n".join(lines)


def stage() -> dict[Path, Path]:
    sources = collect_markdown()
    staged_by_source = {source: staged_path(source) for source in sources}

    collisions = len(set(staged_by_source.values())) != len(staged_by_source)
    if collisions:
        raise SystemExit("error: two documents map to the same staged path")

    if STAGING.exists():
        shutil.rmtree(STAGING)
    STAGING.mkdir(parents=True)

    repo_url = repository_url()
    branch = branch_name()
    assets: set[Path] = set()

    for source, staged in staged_by_source.items():
        text = (ROOT / source).read_text(encoding="utf-8", errors="replace")
        text = rewrite_links(
            text, source, staged, staged_by_source, assets, repo_url, branch
        )
        destination = STAGING / staged
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(text, encoding="utf-8")

    for asset in assets:
        destination = STAGING / asset
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(ROOT / asset, destination)

    (STAGING / "index.md").write_text(landing_page(staged_by_source), encoding="utf-8")

    print(f"staged {len(staged_by_source)} documents and {len(assets)} assets in {STAGING}")
    return staged_by_source


def build() -> None:
    if shutil.which("mkdocs") is None and not _module_available("mkdocs"):
        raise SystemExit(
            "error: mkdocs is not installed.\n"
            "  pip install mkdocs-material\n"
            "  (the GitHub Actions workflow installs it automatically)"
        )
    subprocess.run(
        [sys.executable, "-m", "mkdocs", "build", "--clean", "--strict"],
        cwd=ROOT,
        check=True,
    )
    print(f"built {OUTPUT}")


def _module_available(name: str) -> bool:
    try:
        subprocess.run(
            [sys.executable, "-c", f"import {name}"],
            check=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
    except (OSError, subprocess.CalledProcessError):
        return False
    return True


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--stage-only",
        action="store_true",
        help="collect and rewrite documents without running MkDocs",
    )
    arguments = parser.parse_args()
    stage()
    if not arguments.stage_only:
        build()


if __name__ == "__main__":
    main()
