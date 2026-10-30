#!/usr/bin/env python3
"""Render the M2-17 report charts from the committed CSV data.

No third-party packages: the SVGs are written directly so the charts can be
regenerated on any host with Python 3.

Usage:
    python3 scripts/m2_17_charts.py
"""

from __future__ import annotations

import csv
import html
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DATA = ROOT / "docs" / "profile"

INK = "#1f2430"
MUTED = "#6b7280"
GRID = "#e5e7eb"
TARGET = "#d97706"
BEFORE = "#d9534f"
AFTER = "#2e9e5b"
BLUE = "#4f7cff"


def esc(value: object) -> str:
    return html.escape(str(value), quote=True)


def text(x: float, y: float, value: str, size: int = 13, anchor: str = "start",
         fill: str = INK, weight: str = "normal") -> str:
    return (
        f'<text x="{x:.1f}" y="{y:.1f}" font-family="ui-sans-serif, system-ui, sans-serif" '
        f'font-size="{size}" text-anchor="{anchor}" fill="{fill}" font-weight="{weight}">'
        f"{esc(value)}</text>"
    )


def line(x1: float, y1: float, x2: float, y2: float, stroke: str,
         width: float = 1, dash: str | None = None) -> str:
    extra = f' stroke-dasharray="{dash}"' if dash else ""
    return (
        f'<line x1="{x1:.1f}" y1="{y1:.1f}" x2="{x2:.1f}" y2="{y2:.1f}" '
        f'stroke="{stroke}" stroke-width="{width}"{extra} />'
    )


def rect(x: float, y: float, w: float, h: float, fill: str,
         stroke: str | None = None, rx: float = 0) -> str:
    extra = f' stroke="{stroke}"' if stroke else ""
    return (
        f'<rect x="{x:.1f}" y="{y:.1f}" width="{w:.1f}" height="{h:.1f}" '
        f'rx="{rx}" fill="{fill}"{extra} />'
    )


def circle(x: float, y: float, r: float, fill: str, stroke: str = "#ffffff") -> str:
    return (
        f'<circle cx="{x:.1f}" cy="{y:.1f}" r="{r}" fill="{fill}" '
        f'stroke="{stroke}" stroke-width="2" />'
    )


def document(width: int, height: int, body: list[str]) -> str:
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" '
        f'viewBox="0 0 {width} {height}">\n'
        f'<rect width="{width}" height="{height}" fill="#ffffff" />\n'
        + "\n".join(body)
        + "\n</svg>\n"
    )


def read_matrix(name: str) -> list[dict[str, str]]:
    with (DATA / name).open(newline="") as handle:
        return list(csv.DictReader(handle))


def changed_cells_chart() -> None:
    before = [row for row in read_matrix("m2-17-matrix-before.csv") if row["edge"] == "8"]
    after = [row for row in read_matrix("m2-17-matrix-after.csv") if row["edge"] == "8"]

    width, height = 960, 570
    left, right, top, bottom = 88, 32, 86, 78
    plot_w = width - left - right
    plot_h = height - top - bottom
    x_max, y_max = 60.0, 35.0

    def px(value: float) -> float:
        return left + value / x_max * plot_w

    def py(value: float) -> float:
        return top + (y_max - value) / y_max * plot_h

    body: list[str] = [
        text(left, 34, "One edit's cost vs how much of the chunk it changes", 21, weight="600"),
        text(left, 56, "128³ chunk, 8³ light cells, p95 of 2,000 samples per edit; lower is better", 13, fill=MUTED),
    ]

    for tick in range(0, 61, 10):
        x = px(tick)
        body.append(line(x, top, x, top + plot_h, GRID))
        body.append(text(x, top + plot_h + 22, f"{tick}%", 12, "middle", MUTED))
    for tick in range(0, 36, 5):
        y = py(tick)
        body.append(line(left, y, left + plot_w, y, GRID))
        body.append(text(left - 12, y + 4, f"{tick}%", 12, "end", MUTED))

    body.append(line(left, top + plot_h, left + plot_w, top + plot_h, INK, 1.2))
    body.append(line(left, top, left, top + plot_h, INK, 1.2))
    body.append(text(left + plot_w / 2, height - 30, "padded cells changed by the edit", 13, "middle", MUTED))
    body.append(
        f'<text x="22" y="{top + plot_h / 2:.1f}" font-family="ui-sans-serif, system-ui, sans-serif" '
        f'font-size="13" text-anchor="middle" fill="{MUTED}" '
        f'transform="rotate(-90 22 {top + plot_h / 2:.1f})">incremental p95 / full p95</text>'
    )

    target_y = py(20)
    body.append(line(left, target_y, left + plot_w, target_y, TARGET, 2, "7 5"))
    body.append(text(left + plot_w - 8, target_y - 8, "M2-17 target: 20%", 12, "end", TARGET, "600"))

    def points(rows: list[dict[str, str]]) -> list[tuple[float, float, str]]:
        return [
            (
                float(row["changed"]) / float(row["total"]) * 100.0,
                float(row["ratio_p95"]),
                row["label"],
            )
            for row in rows
        ]

    before_points = points(before)
    after_points = points(after)

    for series, color in ((before_points, BEFORE), (after_points, AFTER)):
        ordered = sorted(series)
        path = " ".join(f"{px(x):.1f},{py(y):.1f}" for x, y, _ in ordered)
        body.append(
            f'<polyline points="{path}" fill="none" stroke="{color}" stroke-width="2" '
            f'opacity="0.35" />'
        )

    offsets = {
        "centre": (10, -10),
        "corner": (-8, -12),
        "border_side": (-8, 18),
        "behind_pillar": (10, -8),
        "sky_gain": (10, 4),
        "opacity_drop": (10, -8),
    }
    names = {
        "centre": "centre",
        "corner": "corner",
        "border_side": "border side",
        "behind_pillar": "behind pillar",
        "sky_gain": "sky gain",
        "opacity_drop": "opacity drop",
    }

    for series, color in ((before_points, BEFORE), (after_points, AFTER)):
        for x, y, label in series:
            body.append(circle(px(x), py(y), 5.5, color))
            dx, dy = offsets[label]
            body.append(text(px(x) + dx, py(y) + dy, names[label], 11, "start", color))

    legend_y = 70
    body.append(circle(width - 300, legend_y, 5, BEFORE))
    body.append(text(width - 288, legend_y + 4, "increase-only baseline", 12, "start", MUTED))
    body.append(circle(width - 150, legend_y, 5, AFTER))
    body.append(text(width - 138, legend_y + 4, "final", 12, "start", MUTED))

    (DATA / "m2-17-changed-cells.svg").write_text(document(width, height, body))
    print("wrote docs/profile/m2-17-changed-cells.svg")


def phase_chart() -> None:
    with (DATA / "m2-17-phases.csv").open(newline="") as handle:
        rows = list(csv.DictReader(handle))

    width, height = 960, 700
    left, right = 210, 90
    bar_w = width - left - right
    top = 92
    row_h = 38
    panel_gap = 150
    top_panel_h = len(rows) * row_h
    bottom_top = top + top_panel_h + panel_gap
    bottom_h = len(rows) * row_h

    colors = ["#8b929c", BLUE, "#3d8bfd", "#3aa0a0", AFTER]
    body: list[str] = [
        text(56, 34, "M2-17 optimization path", 21, weight="600"),
        text(56, 56, "p95 edit cost and the same value as a share of the full pass", 13, fill=MUTED),
        text(56, top - 16, "p95 edit cost (µs)", 14, weight="600"),
    ]

    max_us = 56.0
    for index, (row, color) in enumerate(zip(rows, colors)):
        y = top + index * row_h
        value = float(row["p95_us"])
        body.append(text(left - 16, y + 17, row["phase"], 13, "end"))
        body.append(rect(left, y + 2, bar_w * value / max_us, 24, color, rx=4))
        body.append(text(left + bar_w * value / max_us + 8, y + 19, f"{value:.2f} µs", 12, fill=INK))

    body.append(text(56, bottom_top - 34, "share of a full pass (p95)", 14, weight="600"))
    max_ratio = 135.0
    target_x = left + bar_w * 20.0 / max_ratio
    for index, (row, color) in enumerate(zip(rows, colors)):
        y = bottom_top + index * row_h
        value = float(row["ratio_p95"])
        body.append(text(left - 16, y + 17, row["phase"], 13, "end"))
        body.append(rect(left, y + 2, bar_w * value / max_ratio, 24, color, rx=4))
        body.append(text(left + bar_w * value / max_ratio + 8, y + 19, f"{value:.1f}%", 12))
    body.append(line(target_x, bottom_top - 8, target_x, bottom_top + bottom_h + 4, TARGET, 2, "7 5"))
    body.append(text(target_x + 8, bottom_top - 14, "M2-17 target 20%", 12, fill=TARGET, weight="600"))

    body.append(text(56, height - 28, "p95 from the 10,000-sample bench loop; ratios use the full pass measured in the same run.", 12, fill=MUTED))
    body.append(text(56, height - 10, "The full-pass p95 varies 35–44 µs on this host, so the last two ratios are conservative.", 12, fill=MUTED))

    (DATA / "m2-17-phases.svg").write_text(document(width, height, body))
    print("wrote docs/profile/m2-17-phases.svg")


def main() -> None:
    changed_cells_chart()
    phase_chart()


if __name__ == "__main__":
    main()
