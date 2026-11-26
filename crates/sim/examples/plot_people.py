#!/usr/bin/env python3
"""Charts of the CSVs written by the `people` example.

    python3 crates/sim/examples/plot_people.py <data_dir> <out_prefix>

Writes `<out_prefix>01-age-pyramid-households.png`, `02-activities-by-hour`,
`03-trips-by-purpose`, `04-day-in-the-life`, `05-money-and-rent`.
Same palette and style as `plot_headless.py` (validated categorical order).
"""
import csv
import sys

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402
from matplotlib.patches import Patch  # noqa: E402

SURFACE = "#fcfcfb"
TEXT = "#0b0b0b"
TEXT2 = "#52514e"
GRID = "#e4e3df"
SERIES = ["#2a78d6", "#eb6834", "#1baf7a", "#eda100", "#e87ba4", "#008300", "#6250d6"]

ACTIVITIES = ["sleeping", "home", "working", "shopping", "leisure", "travelling"]
ACT_NAMES = ["Sleeping", "At home", "Working", "Shopping", "Leisure", "Travelling"]
ACT_COLORS = dict(zip(["Sleeping", "Home", "Working", "Shopping", "Leisure", "Travelling"], SERIES))
PURPOSES = ["to_work", "to_home", "move_in", "move_out", "shopping", "goods", "leisure"]
PURPOSE_NAMES = ["To work", "Home from work", "Moving in", "Moving out", "Shopping", "Goods", "Leisure"]
KINDS = ["single", "couple", "family", "seniors_hh"]
KIND_NAMES = ["Single", "Couple", "Family", "Seniors"]


def load(path):
    with open(path) as f:
        return list(csv.DictReader(f))


def style(ax, title):
    ax.set_facecolor(SURFACE)
    ax.set_title(title, loc="left", color=TEXT, fontsize=11, pad=8)
    ax.grid(True, color=GRID, linewidth=0.8)
    ax.set_axisbelow(True)
    for s in ["top", "right"]:
        ax.spines[s].set_visible(False)
    for s in ["left", "bottom"]:
        ax.spines[s].set_color(GRID)
    ax.tick_params(colors=TEXT2, labelsize=9)


def fig(w, h, rows=1, cols=1, **kw):
    f, axes = plt.subplots(rows, cols, figsize=(w, h), **kw)
    f.patch.set_facecolor(SURFACE)
    return f, axes


def hour_axis(ax):
    ax.set_xlim(0, 24)
    ax.set_xticks(range(0, 25, 3))
    ax.set_xticklabels([f"{h:02d}:00" for h in range(0, 25, 3)])


def stacked_area(ax, xs, series, colors, names):
    ys = [0.0] * len(xs)
    for vals, c, n in zip(series, colors, names):
        top = [a + b for a, b in zip(ys, vals)]
        ax.fill_between(xs, ys, top, color=c, linewidth=0, label=n)
        ax.plot(xs, top, color=SURFACE, linewidth=1.2)
        ys = top


def chart_population(data, out):
    days = load(f"{data}/days.csv")
    f, (a1, a2) = fig(13, 5.2, 1, 2, gridspec_kw={"width_ratios": [1, 1.25]})
    bands = [f"a{i}" for i in range(0, 100, 10)]
    labels = [f"{i}-{i + 9}" for i in range(0, 90, 10)] + ["90+"]
    picks = [days[len(days) // 3], days[2 * len(days) // 3], days[-1]]
    hgt = 0.26
    for k, d in enumerate(picks):
        vals = [int(d[b]) for b in bands]
        ys = [i + (k - 1) * hgt for i in range(len(bands))]
        a1.barh(ys, vals, height=hgt - 0.04, color=SERIES[k], label=f"Day {d['day']} ({int(d['population']):,} people)")
    a1.set_yticks(range(len(bands)))
    a1.set_yticklabels(labels)
    a1.set_xlabel("Citizens", color=TEXT2)
    style(a1, "Age pyramid (years)")
    a1.legend(frameon=False, fontsize=9, loc="upper right")
    xs = [int(d["day"]) for d in days]
    series = [[int(d[k]) for d in days] for k in KINDS]
    stacked_area(a2, xs, series, SERIES[:4], KIND_NAMES)
    style(a2, "Households by kind over time")
    a2.set_xlabel("Day", color=TEXT2)
    a2.set_ylabel("Households", color=TEXT2)
    last = days[-1]
    y = 0
    for k, n in zip(KINDS, KIND_NAMES):
        v = int(last[k])
        a2.annotate(f"{n} {v:,}", (xs[-1], y + v / 2), xytext=(4, 0), textcoords="offset points", va="center", fontsize=9, color=TEXT)
        y += v
    a2.set_xlim(xs[0], xs[-1] + 2.5)
    a2.legend(frameon=False, fontsize=9, loc="upper left")
    f.suptitle("Who lives in the city: a grid-player town growing to ~5,000 people", x=0.01, ha="left", color=TEXT, fontsize=13)
    f.tight_layout()
    f.savefig(out, dpi=110, facecolor=SURFACE)


def chart_activities(data, out):
    rows = load(f"{data}/activity.csv")
    xs = [float(r["hour"]) for r in rows]
    f, ax = fig(12, 5.2)
    series = [[float(r[a]) * 100 for r in rows] for a in ACTIVITIES]
    stacked_area(ax, xs, series, SERIES[:6], ACT_NAMES)
    hour_axis(ax)
    ax.set_ylim(0, 100)
    ax.set_ylabel("Share of citizens (%)", color=TEXT2)
    style(ax, "What citizens are doing through one day (share of all citizens, every 10 in-game minutes)")
    ax.legend(frameon=False, fontsize=9, loc="upper center", ncol=6, bbox_to_anchor=(0.5, -0.08))
    f.tight_layout()
    f.savefig(out, dpi=110, facecolor=SURFACE)


def chart_trips(data, out):
    trips = load(f"{data}/trips.csv")
    act = load(f"{data}/activity.csv")
    f, (a1, a2) = fig(12, 8.5, 2, 1, sharex=True)
    bottom = [0] * 24
    for p, n, c in zip(PURPOSES, PURPOSE_NAMES, SERIES):
        vals = [int(r[p]) for r in trips]
        if sum(vals) == 0:
            continue
        a1.bar([h + 0.5 for h in range(24)], vals, bottom=bottom, width=0.9, color=c, label=n, edgecolor=SURFACE, linewidth=1)
        bottom = [a + b for a, b in zip(bottom, vals)]
    style(a1, "Trips started per hour, by purpose (all trips: on the road and off screen)")
    a1.set_ylabel("Trips", color=TEXT2)
    a1.legend(frameon=False, fontsize=9, ncol=4, loc="upper left")
    xs = [float(r["hour"]) for r in act]
    keys = ["v_work", "v_home", "v_movein", "v_moveout", "v_shopping", "v_goods", "v_leisure"]
    for k, n, c in zip(keys, PURPOSE_NAMES, SERIES):
        vals = [int(r[k]) for r in act]
        if max(vals) == 0:
            continue
        a2.plot(xs, vals, color=c, linewidth=2, label=n)
        a2.annotate(n, (xs[-1], vals[-1]), xytext=(4, 0), textcoords="offset points", va="center", fontsize=8, color=TEXT)
    style(a2, "Cars on the road by trip purpose (drawn share of trips)")
    a2.set_ylabel("Cars", color=TEXT2)
    hour_axis(a2)
    a2.set_xlim(0, 26.5)
    a2.legend(frameon=False, fontsize=9, ncol=4, loc="upper left")
    f.tight_layout()
    f.savefig(out, dpi=110, facecolor=SURFACE)


def chart_day_in_life(data, out):
    rows = load(f"{data}/timeline.csv")
    about = {}
    with open(f"{data}/samples.txt") as fh:
        for line in fh:
            parts = line.rstrip("\n").split("|")
            if len(parts) >= 3:
                about[int(parts[0])] = (parts[1], parts[2], parts[3] if len(parts) > 3 else "")
    who = sorted({int(r["who"]) for r in rows})
    f, ax = fig(13, 1.9 + 1.5 * len(who))
    for k, i in enumerate(who):
        pts = [r for r in rows if int(r["who"]) == i]
        y = len(who) - 1 - k
        for a, b in zip(pts, pts[1:] + [None]):
            x0 = float(a["hour"])
            x1 = float(b["hour"]) if b else 24.0
            c = ACT_COLORS.get(a["activity"], GRID)
            ax.barh(y, x1 - x0, left=x0, height=0.5, color=c, linewidth=0)
            if a["on_road"] == "1":
                ax.barh(y - 0.33, x1 - x0, left=x0, height=0.08, color=TEXT2, linewidth=0)
        # Trip labels where a trip starts.
        prev = ""
        for r in pts:
            if r["purpose"] and r["purpose"] != prev:
                ax.annotate(r["purpose"], (float(r["hour"]), y + 0.28), fontsize=7, color=TEXT2, rotation=0)
            prev = r["purpose"]
        label, desc, _ = about.get(i, ("", "", ""))
        ax.annotate(f"{label}: {desc}", (0, y + 0.42), fontsize=9, color=TEXT, annotation_clip=False)
    ax.set_yticks([])
    ax.set_ylim(-0.6, len(who) - 0.2)
    hour_axis(ax)
    style(ax, "A day in the life of three citizens (dark tick under the bar: in a car drawn on the road)")
    ax.grid(axis="y", visible=False)
    ax.legend(
        handles=[Patch(color=ACT_COLORS[k], label=n) for k, n in zip(ACT_COLORS, ACT_NAMES)],
        frameon=False,
        fontsize=9,
        ncol=6,
        loc="upper center",
        bbox_to_anchor=(0.5, -0.12),
    )
    f.tight_layout()
    f.savefig(out, dpi=110, facecolor=SURFACE)


def chart_money(data, out):
    hh = load(f"{data}/households.csv")
    f, (a1, a2) = fig(13, 5, 1, 2)
    edges = [-100, 0, 50, 100, 200, 400, 800, 1600, 3200, 10**9]
    names = ["<0", "0-50", "50-100", "100-200", "200-400", "400-800", "800-1.6k", "1.6k-3.2k", "3.2k+"]
    bottom = [0] * len(names)
    for kind, kn, c in zip(["single", "couple", "family", "seniors"], KIND_NAMES, SERIES):
        counts = [0] * len(names)
        for h in hh:
            if h["kind"] != kind:
                continue
            m = int(h["money"])
            for i in range(len(names)):
                if edges[i] <= m < edges[i + 1]:
                    counts[i] += 1
        a1.bar(range(len(names)), counts, bottom=bottom, color=c, label=kn, edgecolor=SURFACE, linewidth=1)
        bottom = [a + b for a, b in zip(bottom, counts)]
    a1.set_xticks(range(len(names)))
    a1.set_xticklabels(names, rotation=0, fontsize=8)
    a1.set_xlabel("Household money ($)", color=TEXT2)
    a1.set_ylabel("Households", color=TEXT2)
    style(a1, "Household money")
    a1.legend(frameon=False, fontsize=9)
    bnames = ["<15%", "15-30%", "30-45%", "45-60%", "60%+", "no income"]
    bottom = [0] * len(bnames)
    for kind, kn, c in zip(["single", "couple", "family", "seniors"], KIND_NAMES, SERIES):
        counts = [0] * len(bnames)
        for h in hh:
            if h["kind"] != kind:
                continue
            inc, rent = int(h["income"]), int(h["rent"])
            if inc <= 0:
                counts[5] += 1
                continue
            b = rent / inc
            counts[sum(1 for x in [0.15, 0.30, 0.45, 0.60] if b >= x)] += 1
        a2.bar(range(len(bnames)), counts, bottom=bottom, color=c, label=kn, edgecolor=SURFACE, linewidth=1)
        bottom = [a + b for a, b in zip(bottom, counts)]
    a2.set_xticks(range(len(bnames)))
    a2.set_xticklabels(bnames, fontsize=9)
    a2.set_xlabel("Rent / daily income after tax", color=TEXT2)
    style(a2, "Rent burden")
    a2.legend(frameon=False, fontsize=9)
    f.tight_layout()
    f.savefig(out, dpi=110, facecolor=SURFACE)


def main():
    data = sys.argv[1]
    prefix = sys.argv[2]
    chart_population(data, f"{prefix}01-age-pyramid-households.png")
    chart_activities(data, f"{prefix}02-activities-by-hour.png")
    chart_trips(data, f"{prefix}03-trips-by-purpose.png")
    chart_day_in_life(data, f"{prefix}04-day-in-the-life.png")
    chart_money(data, f"{prefix}05-money-and-rent.png")


if __name__ == "__main__":
    main()
