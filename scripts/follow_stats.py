"""Figures from CITYSIM_RECORD_LOG files of follow runs (scripts/follow-check.sh).

For each file: the largest distance on screen (1920x1080 px) between the
followed body and its ring / diamond, and the largest frame-to-frame movement
of the followed body on screen, once the camera has settled (2.5 s after the
follow started: the glide in from up to 200 m away is then done). Walker
runs measure the off-map marker instead of a body. For every run, the largest
change of the camera focus' speed from one frame to the next (m/s; a jump of
the camera shows there as hundreds), after the settle point and during the
glide in (from the second frame of the follow).
"""

import csv
import math
import sys

SETTLE_S = 2.5
PX_PER_M = 1080 / (2 * 70 * math.tan(math.radians(20)))


def num(r, k):
    try:
        return float(r[k])
    except (KeyError, ValueError):
        return math.nan


def max_dv(focus):
    out = 0.0
    for a, b, c in zip(focus, focus[1:], focus[2:]):
        if any(math.isnan(x) for x in a + b + c) or b[2] <= 0 or c[2] <= 0:
            continue
        v1 = ((b[0] - a[0]) / b[2], (b[1] - a[1]) / b[2])
        v2 = ((c[0] - b[0]) / c[2], (c[1] - b[1]) / c[2])
        out = max(out, math.dist(v1, v2))
    return out


def stats(path):
    rows = list(csv.DictReader(open(path)))
    if not rows:
        return None
    walker = "walker" in path
    first = next((r["vehicle"] for r in rows if r["vehicle"] != "-"), "-")
    t = 0.0
    out = {"frames": 0, "ring": 0.0, "dia": 0.0, "move": 0.0, "jerk": 0.0, "ring_m": 0.0, "glide": 0.0}
    prev = None
    focus = []
    glide = []
    for r in rows:
        t = num(r, "t")
        if t < SETTLE_S:
            if t > 0:
                glide.append((num(r, "focus_x"), num(r, "focus_z"), num(r, "dt")))
            continue
        focus.append((num(r, "focus_x"), num(r, "focus_z"), num(r, "dt")))
        if walker:
            p = (num(r, "trav_sx"), num(r, "trav_sy"))
        else:
            if r["vehicle"] != first:
                prev = None
                continue
            p = (num(r, "body_sx"), num(r, "body_sy"))
        if math.isnan(p[0]):
            prev = None
            continue
        out["frames"] += 1
        if not walker:
            for k, m in (("ring", "ring"), ("dia", "dia")):
                q = (num(r, m + "_sx"), num(r, m + "_sy"))
                if not math.isnan(q[0]):
                    out[k] = max(out[k], math.dist(p, q))
            g = (num(r, "ring_x"), num(r, "ring_z"))
            b = (num(r, "body_x"), num(r, "body_z"))
            if not math.isnan(g[0]):
                out["ring_m"] = max(out["ring_m"], math.dist(g, b))
        if prev is not None:
            out["move"] = max(out["move"], math.dist(p, prev))
        prev = p
    out["jerk"] = max_dv(focus)
    out["glide"] = max_dv(glide)
    return out


def main():
    print(f"{'run':<28}{'frames':>7}{'ring px':>10}{'diamond px':>12}{'ring m':>9}{'body move px':>14}{'focus dv m/s':>14}{'glide dv m/s':>14}")
    for path in sys.argv[1:]:
        s = stats(path)
        if not s:
            print(f"{path}: empty")
            continue
        name = path.rsplit("/", 1)[-1][:-4]
        print(
            f"{name:<28}{s['frames']:>7}{s['ring']:>10.2f}{s['dia']:>12.2f}{s['ring_m']:>9.2f}"
            f"{s['move']:>14.3f}{s['jerk']:>14.2f}{s['glide']:>14.2f}"
        )


if __name__ == "__main__":
    main()
