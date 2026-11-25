#!/usr/bin/env python3
"""Chart the CSVs written by the `headless` example.

    python3 crates/sim/examples/plot_headless.py target/headless out.png [title] [before_dir]

`PLOT_ONLY=hierarchy,streets,single_artery` limits the chart to those
scenarios (the round-2 comparison).

With `before_dir` (CSVs of an earlier run), the money panel also shows the
earlier grid player's money as a dashed grey reference line.

Small multiples (one measure per panel, shared x = real minutes at 2.5x):
population, cars on the road, average car commute, money. One line per
scenario, legend plus direct labels at the line ends.
"""
import csv
import os
import sys

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402

SURFACE = "#fcfcfb"
TEXT = "#0b0b0b"
TEXT2 = "#52514e"
GRID = "#e4e3df"
# Validated categorical order (fixed, never cycled).
SERIES = ["#2a78d6", "#eb6834", "#1baf7a", "#eda100", "#e87ba4", "#008300", "#6250d6"]
SCENARIOS = [
    ("hierarchy", "Hierarchy: avenues + streets", "hierarchy"),
    ("streets", "Streets only", "streets only"),
    ("single_artery", "Single artery", "single artery"),
    ("grid", "Grid player", "grid player"),
    ("res_only", "Residential only", "res only"),
    ("two_grid", "Two districts, grid streets", "good layout"),
    ("artery", "Two districts, one junction", "bad layout"),
    ("artery_fixed", "Bad layout, fixed on day 4", "fixed"),
    ("overbuilt", "Overbuilt roads", "overbuilt"),
    ("upfront", "District built on day one", "up front"),
]
# (column, title, scale, label format, source: "hourly" or "daily")
PANELS = [
    ("population", "Population", 1.0, "{:,.0f}", "hourly"),
    ("vehicles", "Cars on the road (every half hour)", 1.0, "{:,.0f}", "hourly"),
    ("peak_vehicles", "Peak cars on the road per day", 1.0, "{:,.0f}", "daily"),
    ("peak_congestion", "Peak congestion per day (% of segments over 80% capacity)", 100.0, "{:.0f}%", "daily"),
    ("avg_commute_s", "Average car commute (s)", 1.0, "{:.0f}", "hourly"),
    ("money", "Money ($ thousands; dashed: grid player before the balance pass)", 0.001, "{:,.0f}k", "hourly"),
]


def load(path):
    with open(path) as f:
        return list(csv.DictReader(f))


def main():
    src = sys.argv[1] if len(sys.argv) > 1 else "target/headless"
    out = sys.argv[2] if len(sys.argv) > 2 else "headless.png"
    title = sys.argv[3] if len(sys.argv) > 3 else "CitySim headless runs"
    before = sys.argv[4] if len(sys.argv) > 4 else None
    before_grid = None
    if before and os.path.exists(os.path.join(before, "grid.csv")):
        before_grid = load(os.path.join(before, "grid.csv"))
    data = {}
    daily = {}
    only = os.environ.get("PLOT_ONLY")
    if only:
        keep = only.split(",")
        SCENARIOS[:] = [s for s in SCENARIOS if s[0] in keep]
    for key, _, _ in SCENARIOS:
        p = os.path.join(src, key + ".csv")
        if os.path.exists(p):
            data[key] = load(p)
        p = os.path.join(src, key + "_days.csv")
        if os.path.exists(p):
            daily[key] = load(p)
    plt.rcParams.update({"font.size": 10, "axes.edgecolor": GRID, "text.color": TEXT, "text.parse_math": False})
    fig, axes = plt.subplots(3, 2, figsize=(14, 13), facecolor=SURFACE)
    for ax, (col, label, scale, fmt, source) in zip(axes.flat, PANELS):
        if col == "money" and not before_grid:
            label = "Money ($ thousands)"
        ax.set_facecolor(SURFACE)
        ax.grid(True, color=GRID, linewidth=0.8)
        ax.set_axisbelow(True)
        for spine in ("top", "right"):
            ax.spines[spine].set_visible(False)
        ax.tick_params(colors=TEXT2)
        ax.set_title(label, loc="left", color=TEXT, fontsize=12, fontweight="bold")
        ends = []
        for i, (key, name, short) in enumerate(SCENARIOS):
            rows = (data if source == "hourly" else daily).get(key)
            if not rows:
                continue
            x = [float(r["minutes"]) for r in rows]
            y = [float(r[col]) * scale for r in rows]
            if col == "avg_commute_s":
                # Hourly samples are noisy; smooth over ~half a day.
                k = 12
                y = [sum(y[max(0, j - k) : j + 1]) / len(y[max(0, j - k) : j + 1]) for j in range(len(y))]
            if source == "daily":
                ax.plot(x, y, color=SERIES[i], linewidth=2, label=name, marker="o", markersize=4)
            else:
                ax.plot(x, y, color=SERIES[i], linewidth=2, label=name, solid_capstyle="round")
            ends.append((y[-1], x[-1], short, SERIES[i]))
        if col == "money" and before_grid:
            x = [float(r["minutes"]) for r in before_grid]
            y = [float(r["money"]) * scale for r in before_grid]
            ax.plot(x, y, color=TEXT2, linewidth=1.5, linestyle=(0, (4, 3)))
            ax.annotate(
                f"grid player before  {fmt.format(y[-1])}",
                (x[-1], y[-1]),
                xytext=(6, 0),
                textcoords="offset points",
                va="center",
                fontsize=8,
                color=TEXT2,
            )
        # Direct labels at line ends, nudged apart.
        ends.sort()
        lo, hi = ax.get_ylim()
        gap = (hi - lo) * 0.065
        placed = []  # (x, label y) of labels already placed
        for y, x, name, color in ends:
            # Only labels ending at about the same x can collide.
            yy = y
            for px, py in sorted(placed, key=lambda p: p[1]):
                if abs(px - x) < 4 and yy - py < gap:
                    yy = py + gap
            placed.append((x, yy))
            ax.plot([x], [y], "o", color=color, markersize=5, markeredgecolor=SURFACE, markeredgewidth=1.5)
            if yy != y:
                # Label pushed away from its line end: a thin leader line.
                ax.plot([x, x + (ax.get_xlim()[1] - ax.get_xlim()[0]) * 0.012], [y, yy], color=GRID, linewidth=1)
            ax.annotate(
                f"{name}  {fmt.format(y)}",
                (x, yy),
                xytext=(10 if yy != y else 6, 0),
                textcoords="offset points",
                va="center",
                fontsize=8,
                color=TEXT2,
            )
        ax.set_xlabel("real minutes at 2.5x", color=TEXT2)
        ax.margins(x=0.02)
        ax.set_xlim(right=max(ax.get_xlim()[1], 1) * 1.28)
        if col == "money":
            ax.axhline(0, color=TEXT2, linewidth=1)
    handles, labels = axes.flat[0].get_legend_handles_labels()
    fig.legend(handles, labels, loc="upper center", ncol=4, frameon=False, bbox_to_anchor=(0.5, 0.965))
    fig.suptitle(title, x=0.01, ha="left", fontsize=15, fontweight="bold", color=TEXT)
    fig.tight_layout(rect=(0, 0, 1, 0.93))
    fig.savefig(out, dpi=110, facecolor=SURFACE)
    print("wrote", out)


if __name__ == "__main__":
    main()
