#!/usr/bin/env python3
"""Table of `scripts/perf-check.sh`'s results: python3 scripts/perf-summary.py DIR.

Reads the benchmark reports in DIR (quality.json, actions10k.json, speed1.json,
speed12.json, plain20k.json, actions20k.json, tools14k.json; missing ones are skipped) and
prints one row per figure: target, result, PASS / FAIL, and for hitches where
the worst frame's time went (main world, render thread, waiting for the GPU)
and what happened in it.
"""
import json
import os
import sys

out = sys.argv[1] if len(sys.argv) > 1 else "."
rows = []


def load(name):
    p = os.path.join(out, name + ".json")
    if not os.path.exists(p):
        return None
    try:
        with open(p) as f:
            return json.load(f)
    except Exception as e:  # a crashed run leaves no or half a file
        rows.append((name, "-", f"unreadable: {e}", "FAIL", ""))
        return None


def where(w):
    if not w:
        return ""
    tags = [t for t in w.get("tags", []) if not t.startswith("action:")]
    parts = f"main {w['main_ms']:.0f} / render {w['render_ms']:.0f} / gpu wait {w.get('gpu_wait_ms', 0):.0f} ms"
    extra = []
    if w.get("meshes"):
        extra.append(f"{w['meshes']} meshes")
    if w.get("new_pipelines"):
        extra.append(f"{w['new_pipelines']} new pipelines")
    s = parts + (", " + ", ".join(extra) if extra else "")
    if tags:
        s += "; " + "; ".join(tags[:3])
    return s


def row(figure, target, value, ok, note=""):
    rows.append((figure, target, value, "PASS" if ok else "FAIL", note))


q = load("quality")
if q:
    sw = q.get("quality_switches", [])
    worst = q.get("quality_switch_worst_ms", {})
    w = max(sw, key=lambda s: s["worst_ms"]) if sw else None
    row("quality switch: worst frame, median of %d" % len(sw), "<= 50 ms", f"{worst.get('median', 0):.1f} ms",
        worst.get("median", 1e9) <= 50)
    row("quality switch: worst frame, worst switch", "<= 50 ms", f"{worst.get('max', 0):.1f} ms",
        worst.get("max", 1e9) <= 50, (f"{w['from']}->{w['to']}: " + where(w.get("worst"))) if w else "")
    for s in sw:
        row(f"quality {s['from']} -> {s['to']}: worst frame", "<= 50 ms", f"{s['worst_ms']:.1f} ms",
            s["worst_ms"] <= 50, f"CPU max main {s.get('main_ms_max', 0):.0f} / render {s.get('render_ms_max', 0):.0f} ms, "
            f"{s['new_pipelines']} new pipelines, shown after {s['ms_to_apply']:.0f} ms")
    applied = sorted(s["ms_to_apply"] for s in sw if s["ms_to_apply"] >= 0)
    if applied:
        row("quality switch: time until shown, median", "info", f"{applied[len(applied) // 2]:.0f} ms", True)
    if len(applied) != len(sw):
        row("quality switch: switches that never applied", "0", str(len(sw) - len(applied)), False)

# The tools' own work per frame (tool systems, their commands, gfx drawing
# their feedback): at most 4 ms in the owner's city (10k and 13.8k runs), 8 ms
# at 20,000 residents with a 20 x 20 grid.
TOOL_ACTS = ("grid", "build the", "undo the", "chain", "zone fill", "zone marquee", "area bulldozer",
             "bulldoze the district")
for name, pop, limit in (("actions10k", "10k", 4.0), ("tools14k", "13.8k", 4.0), ("actions20k", "20k", 8.0)):
    a = load(name)
    if not a:
        continue
    for act in a.get("actions", []):
        tt = act.get("tool_total_ms")
        if not tt or not act["action"].startswith(TOOL_ACTS):
            continue
        row(f"{pop} {act['action']}: tool work per frame, worst", f"<= {limit:.0f} ms",
            f"{tt['max']:.2f} ms (avg {tt['avg']:.2f})", tt["max"] <= limit,
            f"tools {act['tools_ms']['max']:.2f} / commands {act['sim_cmds_ms']['max']:.2f} / "
            f"drawing {act['tool_draw_ms']['max']:.2f} ms")

for name, pop in (("actions10k", "10k"), ("actions20k", "20k"), ("tools14k", "13.8k")):
    a = load(name)
    if not a:
        continue
    target_ok = pop != "20k"
    for act in a.get("actions", []):
        if act["action"] == "end":
            continue
        ok = act["worst_ms"] <= 33.0
        cpu = f"; CPU max main {act.get('main_ms_max', 0):.0f} / render {act.get('render_ms_max', 0):.0f} ms"
        row(f"{pop} {act['action']}: worst frame", "<= 33 ms" if target_ok else "<= 33 ms (info)",
            f"{act['worst_ms']:.1f} ms (avg {act['avg_ms']:.1f})", ok or not target_ok, where(act.get("worst")) + cpu)
    skipped = [t for w in a.get("worst_frames", []) for t in w.get("tags", []) if "skipped" in t]
    if skipped:
        row(f"{pop} actions skipped", "none", skipped[0], False)

s1, s12 = load("speed1"), load("speed12")
if s1 and s12:
    d = s12["frame_ms"]["avg"] - s1["frame_ms"]["avg"]
    row("top speed (12x) minus 1x, average frame", "< 2 ms",
        f"{d:+.2f} ms ({s1['frame_ms']['avg']:.2f} -> {s12['frame_ms']['avg']:.2f})", d < 2.0,
        f"sim per frame {s1['sim_step_ms']['per_frame']:.2f} -> {s12['sim_step_ms']['per_frame']:.2f} ms")
    for s, n in ((s1, "1x"), (s12, "12x")):
        row(f"10k at {n}: worst frame", "<= 33 ms", f"{s['frame_ms']['max']:.1f} ms (p99 {s['frame_ms']['p99']:.1f})",
            s["frame_ms"]["max"] <= 33.0, where((s.get("worst_frames") or [None])[0]))

p20 = load("plain20k")
if p20:
    avg = p20["frame_ms"]["avg"]
    row(f"{p20['population']} residents: average frame", "<= 16.7 ms (60 fps)",
        f"{avg:.2f} ms ({1000 / max(avg, 1e-6):.0f} fps)", avg <= 1000 / 60)
    row(f"{p20['population']} residents: worst frame", "<= 33 ms (info)",
        f"{p20['frame_ms']['max']:.1f} ms (p99 {p20['frame_ms']['p99']:.1f})", True,
        where((p20.get("worst_frames") or [None])[0]))

wf = max(len(r[0]) for r in rows) if rows else 10
wt = max(len(r[1]) for r in rows) if rows else 10
wv = max(len(r[2]) for r in rows) if rows else 10
print(f"{'figure':<{wf}}  {'target':<{wt}}  {'result':<{wv}}  pass  where the worst frame went")
for f, t, v, ok, note in rows:
    print(f"{f:<{wf}}  {t:<{wt}}  {v:<{wv}}  {ok:<4}  {note}")
fails = sum(1 for r in rows if r[3] == "FAIL")
print(f"{len(rows) - fails} passed, {fails} failed")
