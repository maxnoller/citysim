#!/usr/bin/env python3
"""Before / after chart of the scripted cities (daily CSVs of the
`headless` example from two builds).

    python3 crates/sim/examples/plot_compare.py <before_dir> <after_dir> out.png

Small multiples (population, peak cars, peak congestion, money); one colour
per scenario, dashed = before, solid = after; x = real minutes at 2.5x.
"""
import csv
import sys

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402
from matplotlib.lines import Line2D  # noqa: E402

SURFACE = "#fcfcfb"
TEXT = "#0b0b0b"
TEXT2 = "#52514e"
GRID = "#e4e3df"
SERIES = ["#2a78d6", "#eb6834", "#1baf7a", "#eda100"]
SCENARIOS = [
    ("hierarchy", "Hierarchy (avenues + streets)"),
    ("streets", "Streets only"),
    ("single_artery", "Single artery"),
    ("upfront", "District built on day one"),
]
PANELS = [
    ("population", "Population", 1.0, "{:,.0f}"),
    ("peak_vehicles", "Peak cars on the road per day", 1.0, "{:,.0f}"),
    ("peak_congestion", "Peak congestion per day (% of segments over 80%)", 100.0, "{:.0f}%"),
    ("money", "Money ($ thousands)", 0.001, "{:,.0f}k"),
]


def load(path):
    try:
        with open(path) as f:
            return list(csv.DictReader(f))
    except FileNotFoundError:
        return []


def main():
    before, after, out = sys.argv[1], sys.argv[2], sys.argv[3]
    fig, axes = plt.subplots(2, 2, figsize=(14, 9))
    fig.patch.set_facecolor(SURFACE)
    for ax, (col, title, scale, fmt) in zip(axes.flat, PANELS):
        ax.set_facecolor(SURFACE)
        for (name, label), color in zip(SCENARIOS, SERIES):
            for d, style in ((before, "--"), (after, "-")):
                rows = load(f"{d}/{name}_days.csv")
                if not rows:
                    continue
                xs = [float(r["minutes"]) for r in rows]
                ys = [float(r[col]) * scale for r in rows]
                ax.plot(xs, ys, style, color=color, linewidth=2 if style == "-" else 1.5, alpha=1 if style == "-" else 0.75)
                if style == "-":
                    ax.annotate(fmt.format(ys[-1]), (xs[-1], ys[-1]), xytext=(4, 0), textcoords="offset points", fontsize=8, color=TEXT, va="center")
        ax.set_title(title, loc="left", color=TEXT, fontsize=11)
        ax.grid(True, color=GRID, linewidth=0.8)
        for s in ["top", "right"]:
            ax.spines[s].set_visible(False)
        for s in ["left", "bottom"]:
            ax.spines[s].set_color(GRID)
        ax.tick_params(colors=TEXT2, labelsize=9)
        ax.set_xlabel("Real minutes at 2.5x", color=TEXT2, fontsize=9)
    handles = [Line2D([], [], color=c, linewidth=2, label=l) for (_, l), c in zip(SCENARIOS, SERIES)]
    handles += [
        Line2D([], [], color=TEXT2, linewidth=2, linestyle="-", label="Round 3 (citizens)"),
        Line2D([], [], color=TEXT2, linewidth=1.5, linestyle="--", label="Round 2 (before)"),
    ]
    fig.legend(handles=handles, loc="lower center", ncol=6, frameon=False, fontsize=9)
    fig.suptitle("Scripted cities before and after the population model", x=0.01, ha="left", color=TEXT, fontsize=13)
    fig.tight_layout(rect=(0, 0.05, 1, 0.97))
    fig.savefig(out, dpi=100, facecolor=SURFACE)


if __name__ == "__main__":
    main()
