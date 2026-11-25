#!/usr/bin/env python3
"""Before / after charts of the `headless --seeds` sweeps and the `people`
example's week.

    uv run --with matplotlib --with numpy python crates/sim/examples/plot_seeds.py \\
        <before_dir> <after_dir> <people_dir> <out_prefix>

Writes `<out_prefix>NN-name.png`:

* residents and money per real minute for every seed and the main scripted
  cities, before and after;
* cars on the road per 1,000 residents through the day at four city sizes;
* the share of cars with somebody identified in them;
* three citizens' weeks as timelines;
* the budget of a 10,000-resident city by category (per real minute).

The before runs had a 4-minute day, the after runs a 16-minute day: every
x axis is real minutes (at 2.5x for the growth charts) or the hour of the
day, so the two compare directly.
"""
import csv
import glob
import os
import sys

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402
import numpy as np  # noqa: E402

SURFACE = "#fcfcfb"
TEXT = "#0b0b0b"
TEXT2 = "#52514e"
GRID = "#e4e3df"
SERIES = ["#2a78d6", "#eb6834", "#1baf7a", "#eda100", "#e87ba4", "#008300", "#6250d6"]
CITIES = [
    ("hierarchy", "Avenues + streets"),
    ("streets", "Streets only"),
    ("single_artery", "Single artery"),
    ("grid", "Grid player (all zones)"),
]


def load(path):
    with open(path) as f:
        return list(csv.DictReader(f))


def style(ax, title):
    ax.set_facecolor(SURFACE)
    ax.grid(True, color=GRID, linewidth=0.8)
    ax.set_axisbelow(True)
    for s in ("top", "right"):
        ax.spines[s].set_visible(False)
    for s in ("left", "bottom"):
        ax.spines[s].set_color(GRID)
    ax.tick_params(colors=TEXT2)
    ax.set_title(title, loc="left", color=TEXT, fontsize=11, fontweight="bold")


def fig(w, h, rows=1, cols=1, **kw):
    plt.rcParams.update({"font.size": 10, "text.color": TEXT, "text.parse_math": False})
    f, axes = plt.subplots(rows, cols, figsize=(w, h), facecolor=SURFACE, **kw)
    return f, axes


def save(f, out, title, note=None):
    f.suptitle(title, x=0.01, ha="left", fontsize=14, fontweight="bold", color=TEXT)
    if note:
        f.text(0.01, 0.005, note, fontsize=8.5, color=TEXT2, ha="left", va="bottom")
    f.tight_layout(rect=(0, 0.03 if note else 0, 1, 0.95))
    f.savefig(out, dpi=110, facecolor=SURFACE)
    plt.close(f)
    print("wrote", out)


def runs(d, city):
    out = []
    for p in sorted(glob.glob(f"{d}/{city}_seed*_days.csv")):
        seed = int(p.split("_seed")[1].split("_")[0])
        out.append((seed, load(p), load(p.replace("_days", "_hours"))))
    return out


def chart_growth(before, after, out):
    f, axes = fig(15, 9, 2, 2, sharex="col")
    for col, (d, label) in enumerate([(before, "Before (4-minute day)"), (after, "After (16-minute day)")]):
        for row, (key, name, scale) in enumerate(
            [("population", "Residents", 1.0), ("money", "Money ($ thousands)", 0.001)]
        ):
            ax = axes[row][col]
            style(ax, f"{label}: {name}")
            for i, (city, cname) in enumerate(CITIES):
                rs = runs(d, city)
                for k, (seed, days, _) in enumerate(rs):
                    x = [0.0] + [float(r["minutes"]) for r in days]
                    y = [0.0 if key == "population" else 30.0] + [float(r[key]) * scale for r in days]
                    ax.plot(x, y, color=SERIES[i], linewidth=1.2, alpha=0.75, label=cname if k == 0 else None)
            if key == "population":
                ax.axhline(10000, color=TEXT2, linewidth=0.8, linestyle=(0, (4, 3)))
                ax.axhline(5000, color=TEXT2, linewidth=0.8, linestyle=(0, (4, 3)))
                ax.axvline(30, color=TEXT2, linewidth=0.8, linestyle=(0, (2, 3)))
            else:
                ax.axhline(0, color=TEXT2, linewidth=1)
            ax.set_xlim(0, 110)
            if row == 1:
                ax.set_xlabel("real minutes at 2.5x", color=TEXT2)
    axes[0][0].legend(frameon=False, loc="upper left")
    save(
        f,
        out,
        "Residents and money, ten seeds per city, before and after",
        "One line per seed. Dashed: 5,000 and 10,000 residents, 30 minutes. "
        "Before: hierarchy 45 days of 4 min, after: 15 days of 16 min (the same real time).",
    )


def day_rows(hours, target):
    """Hourly rows of the first full day at which the city had reached ~target."""
    days = {}
    for r in hours:
        days.setdefault(int(r["day"]), []).append(r)
    for d in sorted(days):
        rr = days[d]
        if len(rr) >= 24 and int(rr[0]["population"]) >= target:
            return rr[:24]
    return None


def per_1000(d, city, target):
    curves = []
    pops = []
    for _, _, hours in runs(d, city):
        rr = day_rows(hours, target)
        if rr is None:
            continue
        curves.append([float(r["cars_mean"]) * 1000 / max(int(r["population"]), 1) for r in rr])
        pops.append(int(rr[0]["population"]))
    if not curves:
        return None, None
    return np.mean(np.array(curves), axis=0), int(np.mean(pops))


def chart_per_1000(before, after, out):
    f, axes = fig(15, 8.5, 2, 2, sharey=True)
    for ax, target in zip(axes.flat, [500, 1500, 5000, 10000]):
        style(ax, f"About {target:,} residents")
        for d, label, color in [(before, "before", TEXT2), (after, "after", SERIES[0])]:
            city = "hierarchy"
            y, pop = per_1000(d, city, target)
            if y is None:
                continue
            x = np.arange(24) + 0.5
            ax.plot(x, y, color=color, linewidth=2.2, label=f"{label} (avg {pop:,} residents)")
            ax.annotate(f"mean {y.mean():.0f}", (23.5, y[-1]), xytext=(4, 0), textcoords="offset points",
                        fontsize=8, color=color, va="center")
        ax.set_xlim(0, 26)
        ax.set_xticks(range(0, 25, 3))
        ax.set_xlabel("hour of day", color=TEXT2)
        ax.set_ylabel("cars on the road per 1,000", color=TEXT2)
        ax.legend(frameon=False, loc="upper left", fontsize=9)
    save(
        f,
        out,
        "Cars on the road per 1,000 residents through the day (avenue city, mean of ten seeds)",
        "Before: about 9 in 10 of these cars were empty (their people had got out on the calendar). "
        "After: every car carries its driver; trips of up to 150 m are walked.",
    )


def chart_occupied(before, after, out):
    f, ax = fig(12, 5)
    style(ax, "Share of cars on the road with an identified driver, by hour")
    for d, label, color in [(before, "before", TEXT2), (after, "after", SERIES[0])]:
        for i, (city, cname) in enumerate(CITIES[:2]):
            ys = []
            for _, _, hours in runs(d, city):
                rr = day_rows(hours, 5000)
                if rr:
                    ys.append([float(r["occupied_share"]) * 100 for r in rr])
            if not ys:
                continue
            y = np.mean(np.array(ys), axis=0)
            ax.plot(np.arange(24) + 0.5, y, color=color if i == 0 else color, linewidth=2.2,
                    linestyle="-" if i == 0 else (0, (4, 2)), label=f"{label}: {cname} (~5,000 residents)")
    ax.set_ylim(0, 105)
    ax.set_xlim(0, 24)
    ax.set_xticks(range(0, 25, 3))
    ax.set_xlabel("hour of day", color=TEXT2)
    ax.set_ylabel("% of cars (trucks excluded)", color=TEXT2)
    ax.legend(frameon=False, loc="center right")
    save(f, out, "Clicking a car finds somebody in it")


ACTS = ["Sleeping", "Home", "Working", "Shopping", "Leisure", "Travelling"]
ACT_COLORS = {"Sleeping": "#3d4a66", "Home": "#9fb4d9", "Working": SERIES[1], "Shopping": SERIES[3],
              "Leisure": SERIES[4], "Travelling": SERIES[2]}


def chart_week(people, out):
    rows = load(f"{people}/timeline.csv")
    with open(f"{people}/samples.txt") as fh:
        about = [line.rstrip("\n").split("|") for line in fh if line.strip()]
    who = sorted(set(int(r["who"]) for r in rows))
    f, axes = fig(15, 3.2 * len(who), len(who), 1)
    if len(who) == 1:
        axes = [axes]
    for ax, i in zip(axes, who):
        rr = [r for r in rows if int(r["who"]) == i]
        label = about[i][1] if i < len(about) else f"citizen {i}"
        detail = about[i][2] if i < len(about) else ""
        style(ax, f"{label}: {detail}")
        days = sorted(set(int(r["day"]) for r in rr if int(r["day"]) <= 7))
        for d in days:
            dr = [r for r in rr if int(r["day"]) == d]
            for k in range(len(dr)):
                h0 = float(dr[k]["hour"])
                h1 = float(dr[k + 1]["hour"]) if k + 1 < len(dr) else 24.0
                if h1 < h0:
                    h1 = 24.0
                act = dr[k]["activity"]
                color = ACT_COLORS.get(act, "#cccccc")
                if act == "Travelling" and dr[k]["mode"] in ("Walking", "NoCar"):
                    color = "#8fd9b6"
                ax.barh(d, h1 - h0, left=h0, height=0.72, color=color, linewidth=0)
                if act == "Travelling" and dr[k]["on_road"] == "1":
                    ax.barh(d, h1 - h0, left=h0, height=0.25, color="#0b5d3e", linewidth=0)
        ax.set_yticks(days)
        ax.set_yticklabels([f"day {d}" for d in days])
        ax.invert_yaxis()
        ax.set_xlim(0, 24)
        ax.set_xticks(range(0, 25, 2))
    from matplotlib.patches import Patch

    handles = [Patch(color=ACT_COLORS[a], label=a) for a in ACTS]
    handles.append(Patch(color="#8fd9b6", label="Walking / no car (off screen)"))
    handles.append(Patch(color="#0b5d3e", label="In a car on the road"))
    axes[0].legend(handles=handles, frameon=False, ncol=8, loc="lower left", bbox_to_anchor=(0, 1.12), fontsize=9)
    axes[-1].set_xlabel("hour of day", color=TEXT2)
    save(f, out, "A week in the lives of three citizens", "Sampled every 20 ticks (1.5 in-game minutes).")


CATS = [
    ("residential_tax", "Residential tax", "#2a78d6", 1),
    ("commercial_tax", "Commercial tax", "#1baf7a", 1),
    ("industrial_tax", "Industrial tax", "#eda100", 1),
    ("streets", "Street upkeep", "#8a8986", -1),
    ("avenues", "Avenue upkeep", "#5f5e5a", -1),
    ("signals", "Traffic lights", "#3b3a37", -1),
    ("services_base", "Services: base", "#eb6834", -1),
    ("services_city_size", "Services: big city", "#c94a1a", -1),
    ("services_density", "Services: density", "#e87ba4", -1),
]


def budget_at(d, city, target, minutes_per_day):
    for seed, days, _ in runs(d, city):
        for r in days:
            if int(r["population"]) >= target:
                out = {}
                for key, _, _, _ in CATS:
                    if key in r:
                        out[key] = float(r[key]) / minutes_per_day
                if "streets" not in r:
                    out["streets"] = float(r["road_upkeep"]) / minutes_per_day
                    out["services_base"] = float(r["services"]) / minutes_per_day
                return out, int(r["population"]), int(r["money"]), seed
    return None, 0, 0, 0


def chart_budget(before, after, out):
    f, ax = fig(13, 6.5)
    style(ax, "Budget of a 10,000-resident avenue city, dollars per real minute at 1x")
    xs = []
    for x, (d, label, mpd) in enumerate([(before, "Before", 4.0), (after, "After", 16.0)]):
        b, pop, money, seed = budget_at(d, "hierarchy", 10000, mpd)
        if not b:
            continue
        xs.append(x)
        up, down = 0.0, 0.0
        for key, name, color, sign in CATS:
            v = b.get(key, 0.0)
            if v <= 0:
                continue
            if sign > 0:
                ax.bar(x - 0.2, v, 0.38, bottom=up, color=color, label=name if x == 1 or key not in ("streets",) else None)
                up += v
            else:
                ax.bar(x + 0.2, v, 0.38, bottom=down, color=color, label=name)
                down += v
        ax.annotate(
            f"{label} ({pop:,} residents, seed {seed})\nincome ${up:,.0f}/min, costs ${down:,.0f}/min,\n"
            f"net ${up - down:,.0f}/min; ${money:,} in the bank",
            (x, max(up, down)), xytext=(0, 8), textcoords="offset points", ha="center", fontsize=9, color=TEXT2,
        )
    ax.set_xticks([0, 1])
    ax.set_xticklabels(["Before: income | costs", "After: income | costs"])
    hs, ls = ax.get_legend_handles_labels()
    seen = {}
    for h, l in zip(hs, ls):
        seen.setdefault(l, h)
    ax.legend(seen.values(), seen.keys(), frameon=False, loc="upper left", bbox_to_anchor=(1.0, 1.0))
    ax.set_xlim(-0.6, 1.6)
    ax.set_ylim(0, ax.get_ylim()[1] * 1.25)
    save(f, out, "Where the money goes")


def main():
    before, after, people, prefix = sys.argv[1:5]
    chart_growth(before, after, f"{prefix}01-residents-money-10-seeds.png")
    chart_per_1000(before, after, f"{prefix}02-cars-per-1000-by-hour.png")
    chart_occupied(before, after, f"{prefix}03-cars-with-people.png")
    if os.path.exists(f"{people}/timeline.csv"):
        chart_week(people, f"{prefix}04-three-weeks.png")
    chart_budget(before, after, f"{prefix}05-budget-10k.png")


if __name__ == "__main__":
    main()
