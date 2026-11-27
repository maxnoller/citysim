#!/usr/bin/env python3
"""Blocks drawn street by street, played through the real game (real OS
input, see play.py and zplay.py).

Builds hand-drawn blocks on an empty map with the real road tool, one
straight street at a time at distances that are not whole cells and a degree
or two off a right angle (a copy of the owner's hand-drawn grid, a square, a
rectangle, a block with one side 2 degrees off, a trapezoid, a triangle, an
L, a block with an avenue side, two small rings), zones them with the real
zone tool, lets the town grow and takes pictures.

    python3 scripts/bplay.py --ctl DIR --display :N PHASE [ARGS]

Phases: launch EXE, roads, zone, grow DAYS, shots PREFIX OUT N0 SUFFIX,
site NAME [top|low|street], guide OUT (the road tool's parallel guide),
paint OUT (a fast paint stroke), video DAYS, stats.
Positions are in cells of the world grid (floats allowed).
"""

import argparse
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import play  # noqa: E402
import zplay  # noqa: E402
from zplay import click_at, drag, esc, look, top, zmode, zone_tool  # noqa: E402

# name: (centre cell, view distance)
SITES = {
    "owner": ((33, 117), 330.0),
    "square": ((23, 133), 190.0),
    "rect": ((39, 132.5), 210.0),
    "skew": ((64, 122.5), 200.0),
    "trap": ((60, 133.5), 200.0),
    "tri": ((82, 122), 200.0),
    "ell": ((82, 135), 220.0),
    "avenue": ((103, 122), 210.0),
    "rings": ((112, 138), 300.0),
}


def site(name, mode="top"):
    (cx, cy), d = SITES[name]
    if mode == "top":
        top(cx, cy, d)
    elif mode == "street":
        look(cx, cy, d * 0.45, 0.6, 0.5)
    else:
        look(cx, cy, d * 0.85, 0.5, 0.85)


def frame(pts):
    """Top-down view holding every point with room to spare."""
    xs = [p[0] for p in pts]
    ys = [p[1] for p in pts]
    span = max(max(xs) - min(xs), max(ys) - min(ys)) * zplay.C
    top((min(xs) + max(xs)) / 2, (min(ys) + max(ys)) / 2, max(220.0, span * 2.2))


def road(mode, pts, avenue=False):
    """One road through the real road tool (clicks at the points, Enter)."""
    frame(pts)
    play.road_mode(mode)
    want = "Avenue" if avenue else "Street"
    for _ in range(3):
        if play.stats()["road_kind"] == want:
            break
        play.key("r")
    before = play.stats()["segments"]
    for p in pts:
        click_at(*p)
    play.key("Return")
    play.wait_frames(2)
    after = play.stats()["segments"]
    play.log(f"{mode} {pts}: segments {before} -> {after}")
    esc()


def street(pts, avenue=False):
    """One straight street (or avenue) through the points, built at once."""
    road("Straight", pts, avenue=avenue)


# The owner's hand-drawn grid: three streets across, four along, each drawn
# on its own, spaced 10.6-11.6 cells, a little off true.
OWNER_X = [16.0, 27.3, 38.9, 50.2]
OWNER_Y = [117.2, 106.6]


def owner_roads():
    site("owner")
    for k, x in enumerate(OWNER_X):
        street([(x, 128), (x + 0.15 * (k % 2), 106.6 - 0.1 * k)])
    street([(OWNER_X[0], OWNER_Y[0]), (OWNER_X[-1] + 0.1, OWNER_Y[0] + 0.25)])
    # The top street joins the ends of the four.
    street([(x + 0.15 * (k % 2), 106.6 - 0.1 * k) for k, x in enumerate(OWNER_X)])


def roads():
    play.focus()
    play.cmd("money 2000000")
    # Trunk street from the highway stub.
    look(40, 128, 500)
    street([(11, 128), (60, 128)])
    look(110, 128, 500)
    street([(60, 128), (125, 128)])
    owner_roads()
    # A square, 9.3 x 9.2 cells, sides a degree or two off.
    site("square")
    street([(18.2, 128), (18.0, 137.4)])
    street([(27.5, 128), (27.8, 137.1)])
    street([(18.0, 137.4), (27.8, 137.1)])
    # A rectangle, 13.7 x 8 cells.
    site("rect")
    street([(32.4, 128), (32.4, 136.0)])
    street([(46.1, 128), (46.1, 136.3)])
    street([(32.4, 136.0), (46.1, 136.3)])
    # One side 2 degrees off.
    site("skew")
    street([(58.3, 128), (58.3, 117.9)])
    street([(69.6, 128), (69.95, 117.9)])
    street([(58.3, 117.9), (69.95, 117.9)])
    # A trapezoid.
    site("trap")
    street([(52.0, 128), (54.5, 138.5)])
    street([(67.0, 128), (64.8, 138.5)])
    street([(54.5, 138.5), (64.8, 138.5)])
    # A right triangle.
    site("tri")
    street([(76.3, 128), (76.3, 116.0)])
    street([(76.3, 116.0), (89.6, 128)])
    # An L.
    site("ell")
    street([(89.3, 128), (89.3, 134.4)])
    street([(89.3, 134.4), (81.6, 134.4)])
    street([(81.6, 134.4), (81.6, 141.6)])
    street([(81.6, 141.6), (74.2, 141.6)])
    street([(74.2, 141.6), (74.2, 128)])
    # A block with an avenue along one side.
    site("avenue")
    street([(96.4, 128), (96.4, 116.4)], avenue=True)
    street([(109.7, 128), (109.7, 116.4)])
    street([(96.4, 116.4), (109.7, 116.4)])
    # Two small rings on stems.
    site("rings")
    road("Straight", [(102, 128), (102, 131)])
    for c, r in (((102, 135.0), 4.0), ((118, 138.5), 6.5)):
        cx, cy = c
        if c[0] == 118:
            road("Straight", [(118, 128), (118, cy - r)])
        top_ = (cx, cy - r)
        road("Curve", [top_, (cx + r, cy - r), (cx + r, cy)])
        road("Curve", [(cx + r, cy), (cx + r, cy + r), (cx, cy + r)])
        road("Curve", [(cx, cy + r), (cx - r, cy + r), (cx - r, cy)])
        road("Curve", [(cx - r, cy), (cx - r, cy - r), top_])
    play.select_tool("inspect")
    play.log(play.cmd("stats"))
    play.log(play.cmd("zonestats"))


def owner_middles():
    out = []
    for a, b in zip(OWNER_X, OWNER_X[1:]):
        for y0, y1 in ((128, OWNER_Y[0]), (OWNER_Y[0], OWNER_Y[1])):
            out.append(((a + b) / 2, (y0 + y1) / 2))
    return out


def zone():
    """Each block filled with one click (homes; shops and industry in some
    for jobs)."""
    play.focus()
    site("owner")
    mids = owner_middles()
    for k, p in enumerate(mids):
        zone_tool("c" if k == 4 else "r")
        zmode("Fill")
        click_at(*p)
    fills = [
        ("square", "r", (22.9, 132.7)),
        ("rect", "c", (39.2, 132.0)),
        ("skew", "r", (64.0, 123.0)),
        ("trap", "r", (59.7, 133.2)),
        ("tri", "i", (80.0, 124.5)),
        ("ell", "r", (78.0, 132.0)),
        ("avenue", "c", (103.0, 122.0)),
        ("rings", "r", (102.0, 135.0)),
        ("rings", "r", (118.0, 138.5)),
    ]
    for name, z, p in fills:
        site(name)
        zone_tool(z)
        zmode("Fill")
        click_at(*p)
    # Industry along the trunk's far end for jobs.
    look(118, 128, 300)
    zone_tool("i")
    zmode("Frontage")
    click_at(110.5, 126.8)
    play.select_tool("inspect")
    play.log(play.cmd("zonestats"))


def shots(prefix, out, n0, suffix, names=None):
    """Top-down (zone overlay on) and street-level pictures of every site."""
    k = n0
    play.cmd("camera off")
    names = names or list(SITES)
    for name in names:
        zone_tool("r")
        site(name, "top")
        play.wait_frames(4)
        play.shot(os.path.join(out, f"{prefix}-{k:02d}-{suffix}-{name}-zones.png"))
        k += 1
    play.select_tool("inspect")
    for name in names:
        site(name, "low")
        play.wait_frames(4)
        play.shot(os.path.join(out, f"{prefix}-{k:02d}-{suffix}-{name}-street.png"))
        k += 1
    return k


def paint(out, k, suffix):
    """A fast paint stroke (big jumps between frames, as at a low frame
    rate) diagonally across a zoned block: pictured while held and after."""
    play.focus()
    top(12.6, 119.5, 150.0)
    zone_tool("i")
    zmode("Paint")
    # A zig-zag of 45 degree jumps over the empty land west of the grid.
    pts = [(10.3 + (4.8 if k % 2 else 0.0), 125.2 - 4.8 * k) for k in range(3)]
    zplay.move(*pts[0])
    play.press()
    for p in pts[1:]:
        zplay.move(*p)
    play.wait_frames(3)
    play.shot(os.path.join(out, f"zone-{k:02d}-{suffix}-paint-stroke-held.png"))
    play.release()
    play.wait_frames(3)
    play.select_tool("inspect")
    zone_tool("r")
    play.wait_frames(3)
    play.shot(os.path.join(out, f"zone-{k + 1:02d}-{suffix}-paint-stroke-done.png"))


def guide(out, k, suffix):
    """The road tool drawing a street parallel to another: the guide offers
    the distances that leave whole rows of lots."""
    play.focus()
    top(26.0, 101.0, 200.0)
    play.road_mode("Straight")
    for _ in range(3):
        if play.stats()["road_kind"] == "Street":
            break
        play.key("r")
    click_at(19.0, 99.6)
    for p in [(24.0, 99.55), (29.0, 99.5), (33.0, 99.5)]:
        zplay.move(*p)
    play.wait_frames(4)
    play.shot(os.path.join(out, f"zone-{k:02d}-{suffix}-road-parallel-guide.png"))
    esc()
    esc()
    play.select_tool("inspect")


def owner_views(out, k, suffix):
    """The places of the owner's three screenshots: the hand-drawn grid
    seen at an angle with the zone tool open (rectangle mode), closer on one
    block, and with the traffic overlay."""
    play.focus()
    play.cmd("camera off")
    zone_tool("r")
    zmode("Marquee")
    shots = [
        ("grid", (33.0, 119.0, 175.0, 0.25, 0.95)),
        ("block", (24.0, 112.0, 105.0, 0.2, 0.9)),
    ]
    for name, (x, y, d, yaw, pitch) in shots:
        look(x, y, d, yaw, pitch)
        play.wait_frames(5)
        play.shot(os.path.join(out, f"zone-{k:02d}-{suffix}-owner-{name}.png"))
        k += 1
    for _ in range(4):
        if play.stats()["overlay"] == "Traffic":
            break
        play.key("o")
    look(33.0, 119.0, 175.0, 0.25, 0.95)
    play.wait_frames(5)
    play.shot(os.path.join(out, f"zone-{k:02d}-{suffix}-owner-traffic.png"))
    for _ in range(4):
        if play.stats()["overlay"] == "None":
            break
        play.key("o")
    play.select_tool("inspect")
    return k + 1


def video_play():
    """Real play for the video on a new map: a block drawn street by street
    by hand, zoned with one fill, growing; then a small ring the same way.
    The camera stays put while drawing (clicks land where they are aimed)."""
    play.focus()
    play.cmd("money 2000000")
    look(35, 124, 400, 0.15, 1.05)
    play.wait_frames(8)

    def draw(pts, mode="Straight"):
        play.road_mode(mode)
        before = play.stats()["segments"]
        for p in pts:
            click_at(*p)
        play.key("Return")
        play.wait_frames(3)
        play.log(f"{mode} {pts}: segments {before} -> {play.stats()['segments']}")

    # The trunk, then a block of three streets off it, each on its own, a
    # little off true and not on whole cells.
    draw([(11, 128), (60, 128)])
    draw([(17.3, 128), (17.1, 117.4)])
    draw([(29.6, 128), (29.95, 117.1)])
    draw([(17.1, 117.4), (29.95, 117.1)])
    esc()
    play.wait_frames(4)
    # The whole block with one click; shops along the trunk's far side.
    zone_tool("r")
    zmode("Fill")
    click_at(23.5, 122.6)
    zone_tool("c")
    zmode("Frontage")
    click_at(22.0, 129.4)
    zone_tool("i")
    click_at(40.0, 126.6)
    play.select_tool("inspect")
    play.wait_frames(4)
    play.key("space")
    play.key("4")
    play.log(play.cmd("until-day 3 10", timeout=3000))
    play.key("space")
    # A small ring on a stem south of the trunk, its middle filled.
    look(50, 142, 300, 0.15, 1.15)
    play.wait_frames(6)
    draw([(50, 128), (50, 131)])
    cx, cy, r = 50.0, 137.5, 6.5
    draw([(cx, cy - r), (cx + r, cy - r), (cx + r, cy)], "Curve")
    draw([(cx + r, cy), (cx + r, cy + r), (cx, cy + r)], "Curve")
    draw([(cx, cy + r), (cx - r, cy + r), (cx - r, cy)], "Curve")
    draw([(cx - r, cy), (cx - r, cy - r), (cx, cy - r)], "Curve")
    esc()
    zone_tool("r")
    zmode("Fill")
    click_at(cx, cy)
    zmode("Frontage")
    click_at(cx + r + 1.2, cy)
    play.select_tool("inspect")
    play.wait_frames(4)
    play.key("space")
    play.key("4")
    play.log(play.cmd("until-day 5 10", timeout=3000))
    play.key("space")
    look(36, 130, 260, 0.6, 0.75)
    play.wait_frames(12)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--ctl", default="/tmp/ctl")
    ap.add_argument("--display", default=":0")
    ap.add_argument("phase")
    ap.add_argument("args", nargs="*")
    a = ap.parse_args()
    play.CTL, play.DISPLAY = a.ctl, a.display
    if a.phase == "launch":
        zplay.launch(a.args[0])
        if len(a.args) > 1 and a.args[1] == "load":
            play.focus()
            play.key("F9")
            play.wait_frames(8)
            play.log(play.cmd("stats"))
    elif a.phase == "roads":
        roads()
    elif a.phase == "zone":
        zone()
    elif a.phase == "grow":
        play.focus()
        play.key("space")
        print(play.cmd(f"until-day {a.args[0]} 60", timeout=5400))
        play.key("space")
    elif a.phase == "shots":
        names = a.args[4].split(",") if len(a.args) > 4 else None
        print(shots(a.args[0], a.args[1], int(a.args[2]), a.args[3], names))
    elif a.phase == "guide":
        guide(a.args[0], int(a.args[1]), a.args[2])
    elif a.phase == "video":
        video_play()
    elif a.phase == "owner":
        print(owner_views(a.args[0], int(a.args[1]), a.args[2]))
    elif a.phase == "paint":
        paint(a.args[0], int(a.args[1]), a.args[2])
    elif a.phase == "site":
        site(a.args[0], a.args[1] if len(a.args) > 1 else "top")
    elif a.phase == "shot":
        play.shot(a.args[0])
    elif a.phase == "stats":
        print(play.cmd("stats"))
        print(play.cmd("zonestats"))
    else:
        sys.exit("unknown phase")


if __name__ == "__main__":
    main()
