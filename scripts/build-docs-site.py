#!/usr/bin/env python3
"""Build the documentation site from every Markdown file in the repository.

The repository keeps each document next to the code it describes. This script
collects them into a single staging tree (``.docs-build/``) without moving the
sources, generates a categorized landing page and one index page per section,
rewrites cross-document links, and renders the result into ``site/`` with
MkDocs. GitHub Pages publishes ``site/`` through ``.github/workflows/docs.yml``.

Navigation is curated in ``docs/site.toml``. Anything not listed there still
appears under "Other", so a new document can never silently disappear from the
site; the build prints a note so the taxonomy can be updated.

Usage:
    python3 scripts/build-docs-site.py            # stage + build site/
    python3 scripts/build-docs-site.py --stage-only
"""

from __future__ import annotations

import argparse
import fnmatch
import json
import os
import posixpath
import re
import shutil
import subprocess
import sys
import tomllib
from collections import Counter, defaultdict
from dataclasses import dataclass, field
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
STAGING = ROOT / ".docs-build"
OUTPUT = ROOT / "site"
TAXONOMY = ROOT / "docs" / "site.toml"
GENERATED_CONFIG = ROOT / ".docs-build-mkdocs.yml"

# Build outputs and tool caches never contain documents. GitHub community
# files (`.github/`) are repository plumbing, not documentation pages.
EXCLUDE_DIRS = {
    ".git",
    ".github",
    ".docs-build",
    "assets",
    "__archive__",
    "__sandbox__",
    "__ai-session-history__",
    "mutants.out",
    "mutants.out.old",
    "node_modules",
    "site",
    "target",
}

# Repository-root files that are not documentation pages. AGENTS.md is agent
# instructions and README.md is the GitHub landing page; neither belongs in the
# published site (a root README would also collide with the generated index).
EXCLUDE_ROOT_FILES = {"AGENTS.md", "README.md"}

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

LINK_RE = re.compile(r"(!?\[)([^\]]*)(\]\()([^)\s]+)((?:\s+\"[^\"]*\")?\))")
TITLE_RE = re.compile(r"^#\s+(.+?)\s*$", re.MULTILINE)
SCHEME_RE = re.compile(r"^[a-zA-Z][a-zA-Z0-9+.-]*:")


@dataclass
class Node:
    """One curated navigation section or subsection."""

    id: str
    title: str
    description: str = ""
    pages: list[Path] = field(default_factory=list)
    match: list[str] = field(default_factory=list)
    group_by: str | None = None
    area: str = "reference"
    children: list["Node"] = field(default_factory=list)


def is_excluded(rel: Path) -> bool:
    if any(part in EXCLUDE_DIRS for part in rel.parts):
        return True
    return len(rel.parts) == 1 and rel.name in EXCLUDE_ROOT_FILES


def collect_markdown() -> list[Path]:
    found: list[Path] = []
    # Prune excluded directories instead of walking into them and filtering
    # afterwards: `target/` alone holds tens of thousands of paths, and on old
    # Pythons `rglob` follows symlinked directories.
    for dirpath, dirnames, filenames in os.walk(ROOT, followlinks=False):
        dirnames[:] = [name for name in dirnames if name not in EXCLUDE_DIRS]
        for name in filenames:
            if not name.endswith(".md"):
                continue
            rel = Path(dirpath, name).relative_to(ROOT)
            if not is_excluded(rel):
                found.append(rel)
    return sorted(found)


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


def load_taxonomy() -> list[Node]:
    data = tomllib.loads(TAXONOMY.read_text(encoding="utf-8"))

    def build(raw: dict) -> Node:
        node = Node(
            id=raw["id"],
            title=raw["title"],
            description=raw.get("description", ""),
            pages=[Path(page) for page in raw.get("pages", [])],
            match=raw.get("match", []),
            group_by=raw.get("group_by"),
            area=raw.get("area", "reference"),
        )
        node.children = [build(child) for child in raw.get("child", [])]
        return node

    return [build(raw) for raw in data.get("section", [])]


def load_areas() -> list[tuple[str, str, str]]:
    """Top-level areas, in file order: (id, title, description)."""
    data = tomllib.loads(TAXONOMY.read_text(encoding="utf-8"))
    return [
        (area_id, raw.get("title", area_id), raw.get("description", ""))
        for area_id, raw in data.get("area", {}).items()
    ]


def walk_nodes(nodes: list[Node]):
    for node in nodes:
        yield node
        yield from walk_nodes(node.children)


def classify(sources: list[Path], taxonomy: list[Node]) -> dict[Path, Node]:
    """Place every document in exactly one node: explicit pages, then globs."""
    known = set(sources)
    placement: dict[Path, Node] = {}

    for node in walk_nodes(taxonomy):
        for page in node.pages:
            if page in known and page not in placement:
                placement[page] = node

    missing = [
        (node.id, page)
        for node in walk_nodes(taxonomy)
        for page in node.pages
        if page not in known
    ]
    if missing:
        detail = ", ".join(f"{node}:{page}" for node, page in missing)
        print(
            f"note: docs/site.toml lists {len(missing)} missing document(s): {detail}",
            file=sys.stderr,
        )

    for node in walk_nodes(taxonomy):
        for pattern in node.match:
            for source in sources:
                if source in placement:
                    continue
                if fnmatch.fnmatchcase(source.as_posix(), pattern):
                    placement[source] = node

    leftovers = [source for source in sources if source not in placement]
    if leftovers:
        other = Node(
            id="other",
            title="Other",
            description="Documents not yet placed in docs/site.toml.",
        )
        taxonomy.append(other)
        for source in sorted(leftovers, key=sort_key):
            placement[source] = other
        print(
            f"note: {len(leftovers)} document(s) not in docs/site.toml, "
            f"placed under 'Other': {', '.join(str(s) for s in leftovers[:5])}"
            + (" ..." if len(leftovers) > 5 else ""),
            file=sys.stderr,
        )
    return placement


def unique_labels(sources: list[Path], title_of: dict[Path, str]) -> dict[Path, str]:
    """Disambiguate sibling pages that share an H1, so nav keys stay unique."""
    counts: dict[tuple[Path, str], int] = defaultdict(int)
    for source in sources:
        counts[(source.parent, title_of[source])] += 1
    labels: dict[Path, str] = {}
    for source in sources:
        title = title_of[source]
        labels[source] = (
            f"{title} ({source.stem})" if counts[(source.parent, title)] > 1 else title
        )
    return labels


def node_sources(node: Node, placement: dict[Path, Node]) -> list[Path]:
    """All sources assigned directly to a node, in curated/numeric order."""
    assigned = [source for source, owner in placement.items() if owner is node]
    by_source = set(assigned)
    ordered: list[Path] = []
    for page in node.pages:
        if page in by_source:
            ordered.append(page)
    rest = [source for source in assigned if source not in set(ordered)]
    ordered.extend(sorted(rest, key=sort_key))
    return ordered


def descendant_sources(node: Node, placement: dict[Path, Node]) -> list[Path]:
    rows = node_sources(node, placement)
    for child in node.children:
        rows.extend(descendant_sources(child, placement))
    return rows


def grouped_sources(node: Node, sources: list[Path]) -> list[tuple[Path, list[Path]]]:
    """Group by the first path component below the node's matched root."""
    if node.group_by != "directory":
        return [(Path("."), sources)]
    prefixes = [
        Path(re.split(r"[*?\[]", pattern, maxsplit=1)[0].rstrip("/"))
        for pattern in node.match
    ]
    groups: dict[Path, list[Path]] = defaultdict(list)
    for source in sources:
        key = source.parent
        for prefix in prefixes:
            try:
                relative = source.relative_to(prefix)
            except ValueError:
                continue
            if relative.parts:
                key = prefix / relative.parts[0]
                break
        groups[key].append(source)
    return [
        (key, sorted(groups[key], key=sort_key))
        for key in sorted(groups, key=sort_key)
    ]


def page_entry(
    source: Path,
    labels: dict[Path, str],
    staged_by_source: dict[Path, Path],
) -> dict[str, str]:
    return {labels[source]: staged_by_source[source].as_posix()}


def group_page_entries(
    sources: list[Path],
    root: Path,
    labels: dict[Path, str],
    staged_by_source: dict[Path, Path],
) -> list[dict[str, str]]:
    """Page entries for one nav group, disambiguated by relative path if needed."""
    counts = Counter(labels[source] for source in sources)
    entries: list[dict[str, str]] = []
    for source in sources:
        label = labels[source]
        if counts[label] > 1:
            relative = source.relative_to(root).with_suffix("").as_posix()
            label = f"{label} — {relative}"
        entries.append({label: staged_by_source[source].as_posix()})
    return entries


def nav_for_node(
    node: Node,
    placement: dict[Path, Node],
    labels: dict[Path, str],
    staged_by_source: dict[Path, Path],
) -> list:
    """Nav subtree for a section: nested groups, or a flat list of pages."""
    if node.children:
        entries: list = []
        for child in node.children:
            child_sources = node_sources(child, placement)
            if not child_sources:
                continue
            entries.append(
                {
                    child.title: [
                        page_entry(source, labels, staged_by_source)
                        for source in child_sources
                    ]
                }
            )
        return entries

    sources = node_sources(node, placement)
    entries: list = []
    if node.group_by == "directory":
        for directory, grouped in grouped_sources(node, sources):
            entries.append(
                {
                    directory.name: group_page_entries(
                        grouped, directory, labels, staged_by_source
                    )
                }
            )
    else:
        entries.extend(page_entry(source, labels, staged_by_source) for source in sources)
    return entries


def build_nav(
    taxonomy: list[Node],
    placement: dict[Path, Node],
    labels: dict[Path, str],
    staged_by_source: dict[Path, Path],
) -> list:
    nav: list = [{"Home": "index.md"}]
    for section in taxonomy:
        nav.append({section.title: nav_for_node(section, placement, labels, staged_by_source)})
    return nav


def nav_paths(nav: list):
    for item in nav:
        if isinstance(item, str):
            yield item
            continue
        (_label, value), *_ = item.items()
        if isinstance(value, str):
            yield value
        else:
            yield from nav_paths(value)


def validate_nav(nav: list, staged_by_source: dict[Path, Path]) -> None:
    """Every document must appear exactly once, and only real pages may appear."""
    listed = [path for path in nav_paths(nav) if path != "index.md"]
    expected = {staged.as_posix() for staged in staged_by_source.values()}
    counts = Counter(listed)
    duplicates = sorted(path for path, count in counts.items() if count > 1)
    missing = sorted(expected - set(listed))
    unknown = sorted(set(listed) - expected)
    if duplicates or missing or unknown:
        details = []
        if duplicates:
            details.append(f"duplicated: {', '.join(duplicates)}")
        if missing:
            details.append(f"missing: {', '.join(missing)}")
        if unknown:
            details.append(f"unknown: {', '.join(unknown)}")
        raise SystemExit("error: generated navigation is inconsistent — " + "; ".join(details))


def yaml_string(value: str) -> str:
    # JSON strings are valid YAML double-quoted scalars.
    return json.dumps(value, ensure_ascii=False)


def render_nav(items: list, indent: int = 0) -> list[str]:
    lines: list[str] = []
    pad = "  " * indent
    for item in items:
        if isinstance(item, str):
            lines.append(f"{pad}- {yaml_string(item)}")
            continue
        (label, value), *_ = item.items()
        if isinstance(value, str):
            lines.append(f"{pad}- {yaml_string(label)}: {yaml_string(value)}")
        else:
            lines.append(f"{pad}- {yaml_string(label)}:")
            lines.extend(render_nav(value, indent + 1))
    return lines


def write_generated_config(nav: list) -> None:
    lines = [
        "# Generated by scripts/build-docs-site.py — do not edit or commit.",
        "INHERIT: mkdocs.yml",
        "nav:",
        *render_nav(nav, 1),
        "",
    ]
    GENERATED_CONFIG.write_text("\n".join(lines), encoding="utf-8")


def _section_block(
    section: Node,
    level: int,
    placement: dict[Path, Node],
    labels: dict[Path, str],
    staged_by_source: dict[Path, Path],
) -> list[str]:
    sources = descendant_sources(section, placement)
    if not sources:
        return []
    prefix = "#" * level
    lines = [f"{prefix} {section.title}", ""]
    if section.description:
        lines += [section.description, ""]
    lines += ["| Document | Source |", "|---|---|"]
    for source in sources:
        label = labels[source].replace("|", "\\|")
        lines.append(
            f"| [{label}]({staged_by_source[source].as_posix()}) "
            f"| `{source.as_posix()}` |"
        )
    lines.append("")
    return lines


def landing_page(
    taxonomy: list[Node],
    areas: list[tuple[str, str, str]],
    placement: dict[Path, Node],
    labels: dict[Path, str],
    staged_by_source: dict[Path, Path],
) -> str:
    lines = [
        "# Voxel Engine documentation",
        "",
        "The repository has two documentation areas. **Guides** explain the engine",
        "in plain language; **Reference** holds the precise specifications, decision",
        "records and measured reports. Both are generated from `docs/site.toml`; the",
        "sources stay next to the code they describe.",
        "",
    ]
    repo_url = repository_url()
    if repo_url:
        lines += [f"Source: [{repo_url.removeprefix('https://')}]({repo_url})", ""]

    if not areas:
        for section in taxonomy:
            lines += _section_block(section, 2, placement, labels, staged_by_source)
        return "\n".join(lines)

    rendered: set[str] = set()
    for area_id, title, description in areas:
        sections = [section for section in taxonomy if section.area == area_id]
        if not sections:
            continue
        lines += [f"## {title}", ""]
        if description:
            lines += [description, ""]
        for section in sections:
            rendered.add(section.id)
            lines += _section_block(section, 3, placement, labels, staged_by_source)

    leftover = [section for section in taxonomy if section.id not in rendered]
    if leftover:
        lines += ["## Other", ""]
        for section in leftover:
            lines += _section_block(section, 3, placement, labels, staged_by_source)
    return "\n".join(lines)


def check_links(staged_by_source: dict[Path, Path]) -> None:
    """Fail if a link to another document was not rewritten to a real page."""
    staged_files = set(staged_by_source.values())
    broken: list[tuple[Path, str]] = []
    for source, staged in staged_by_source.items():
        text = (STAGING / staged).read_text(encoding="utf-8", errors="replace")
        for match in LINK_RE.finditer(text):
            target = match.group(4)
            if not target or target.startswith(("#", "//")) or SCHEME_RE.match(target):
                continue
            path = target.partition("#")[0]
            if not path or Path(path).suffix.lower() != ".md":
                continue
            resolved = Path(
                posixpath.normpath(posixpath.join(staged.parent.as_posix(), path))
            )
            if resolved not in staged_files:
                broken.append((source, target))
    if broken:
        detail = "\n".join(f"  {source}: {target}" for source, target in broken)
        raise SystemExit(f"error: broken document links in the staged site:\n{detail}")


def stage() -> dict[Path, Path]:
    sources = collect_markdown()
    staged_by_source = {source: staged_path(source) for source in sources}

    collisions = len(set(staged_by_source.values())) != len(staged_by_source)
    if collisions:
        raise SystemExit("error: two documents map to the same staged path")

    taxonomy = load_taxonomy()
    areas = load_areas()
    placement = classify(sources, taxonomy)
    title_of = {source: document_title(source) for source in sources}
    labels = unique_labels(sources, title_of)

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

    (STAGING / "index.md").write_text(
        landing_page(taxonomy, areas, placement, labels, staged_by_source),
        encoding="utf-8",
    )

    check_links(staged_by_source)
    nav = build_nav(taxonomy, placement, labels, staged_by_source)
    validate_nav(nav, staged_by_source)
    write_generated_config(nav)

    print(
        f"staged {len(staged_by_source)} documents, {len(taxonomy)} sections and "
        f"{len(assets)} assets in {STAGING}"
    )
    return staged_by_source


def build() -> None:
    if shutil.which("mkdocs") is None and not _module_available("mkdocs"):
        raise SystemExit(
            "error: mkdocs is not installed.\n"
            "  pip install mkdocs-material\n"
            "  (the GitHub Actions workflow installs it automatically)"
        )
    subprocess.run(
        [
            sys.executable,
            "-m",
            "mkdocs",
            "build",
            "--clean",
            "--strict",
            "--config-file",
            str(GENERATED_CONFIG),
        ],
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
