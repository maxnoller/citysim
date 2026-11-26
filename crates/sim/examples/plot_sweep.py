"""Charts of headless seed sweeps (`headless -- --seeds N`), before and after.

    uv run --with matplotlib --with numpy python plot_sweep.py OUT.png \
        "before=DIR_A" "after=DIR_B" [--scenarios hierarchy,streets,...]

Per scenario (rows): residents, money, average commute and the share of car
trips given up per day, as the mean over seeds with the min-max band.
"""

import csv
import glob
import os
import sys

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402
import numpy as np  # noqa: E402

COLORS = ["#8a8f98", "#2b7bd6", "#d9622b", "#2ba36b"]


def load(directory, scenario):
    runs = []
    for path in sorted(glob.glob(os.path.join(directory, f"{scenario}_seed*_days.csv"))):
        with open(path) as f:
            rows = list(csv.DictReader(f))
        if rows:
            runs.append(rows)
    return runs


def series(runs, key):
    n = min(len(r) for r in runs)
    out = []
    for r in runs:
        vals = []
        for row in r[:n]:
            if key == "gave_up_share":
                trips = float(row.get("trips", 0) or 0)
                vals.append(100.0 * float(row.get("given_up", 0) or 0) / max(trips, 1.0))
            else:
                vals.append(float(row[key]))
        out.append(vals)
    return np.array(out)


def main():
    out = sys.argv[1]
    sets = []
    scenarios = None
    args = sys.argv[2:]
    i = 0
    while i < len(args):
        a = args[i]
        if a == "--scenarios":
            scenarios = args[i + 1].split(",")
            i += 2
            continue
        label, d = a.split("=", 1)
        sets.append((label, d))
        i += 1
    if scenarios is None:
        names = set()
        for _, d in sets:
            for p in glob.glob(os.path.join(d, "*_seed*_days.csv")):
                names.add(os.path.basename(p).split("_seed")[0])
        scenarios = sorted(names)
    metrics = [
        ("population", "residents"),
        ("money", "money ($)"),
        ("avg_commute_s", "avg commute (s)"),
        ("gave_up_share", "cars given up (% of trips)"),
    ]
    fig, axes = plt.subplots(len(scenarios), len(metrics), figsize=(4.2 * len(metrics), 2.6 * len(scenarios)), squeeze=False)
    for r, sc in enumerate(scenarios):
        for c, (key, title) in enumerate(metrics):
            ax = axes[r][c]
            for k, (label, d) in enumerate(sets):
                runs = load(d, sc)
                if not runs:
                    continue
                y = series(runs, key)
                x = np.arange(1, y.shape[1] + 1)
                col = COLORS[k % len(COLORS)]
                ax.fill_between(x, y.min(axis=0), y.max(axis=0), color=col, alpha=0.18, linewidth=0)
                ax.plot(x, y.mean(axis=0), color=col, lw=1.8, label=f"{label} ({len(runs)} seeds)")
            if r == 0:
                ax.set_title(title, fontsize=10)
            if c == 0:
                ax.set_ylabel(sc, fontsize=10, fontweight="bold")
            ax.grid(alpha=0.25)
            ax.tick_params(labelsize=8)
            if r == len(scenarios) - 1:
                ax.set_xlabel("day (16 real minutes each at 1x)", fontsize=8)
            if r == 0 and c == 0:
                ax.legend(fontsize=8, loc="upper left")
    fig.tight_layout()
    fig.savefig(out, dpi=110)
    print("wrote", out)


if __name__ == "__main__":
    main()
