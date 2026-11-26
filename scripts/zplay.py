#!/usr/bin/env python3
"""Zoning test bed played through the real game (real OS input, see play.py).

Builds a set of awkward roads on an empty map with the real road tool
(gentle and tight curves, an S-bend, a ring, a curved cul-de-sac, a curved
street leaving a grid, a curved avenue, rotated grids), zones them with the
real zone tool in every mode, lets the town grow and takes pictures.

    python3 scripts/zplay.py --ctl DIR --display :N PHASE [ARGS]

Phases: roads, zone, grow DAYS, shots PREFIX, site NAME (camera), stats.
Positions are in cells of the world grid (floats allowed); the control
channel's `screenm` turns them into window pixels.
"""

import argparse
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import play  # noqa: E402

C = 8.0


def m(x, y):
    """Cell coordinates (floats; integer = cell centre) to sim metres."""
    return ((x + 0.5) * C, (y + 0.5) * C)


def px(x, y):
    mx, my = m(x, y)
    sx, sy, where = play.cmd(f"screenm {mx} {my}").split()
    if where != "inside":
        raise RuntimeError(f"({x},{y}) off screen")
    return float(sx), float(sy)


def move(x, y):
    play.move_px(*px(x, y))
    play.wait_frames()


def click_at(x, y):
    move(x, y)
    play.click()


def drag(path, hold=1):
    """Press at the first point, move through the others, release."""
    move(*path[0])
    play.press()
    for p in path[1:]:
        move(*p)
    play.wait_frames(hold)
    play.release()


def look(cx, cy, dist=300.0, yaw=0.0, pitch=1.2):
    mx, my = m(cx, cy)
    play.cmd(f"camera {mx} {my} {yaw} {pitch} {dist}")
    play.wait_frames(3)


def top(cx, cy, dist=300.0):
    look(cx, cy, dist, 0.0, 1.45)


def street(cx, cy, dist=70.0, yaw=0.6):
    look(cx, cy, dist, yaw, 0.45)


def esc():
    play.key("Escape")


def frame(pts):
    """Top-down view holding every point."""
    xs = [p[0] for p in pts]
    ys = [p[1] for p in pts]
    span = max(max(xs) - min(xs), max(ys) - min(ys)) * C
    top((min(xs) + max(xs)) / 2, (min(ys) + max(ys)) / 2, max(200.0, span * 1.5))


def road(mode, pts, avenue=False):
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


def zone_tool(z):
    play.select_tool({"r": "zone-r", "c": "zone-c", "i": "zone-i", "none": "dezone"}[z])


def zmode(name):
    play.set_mode("zone_mode", name)


# ------------------------------------------------------------------ layout

SITES = {
    # name: (centre cell, view distance)
    "gentle": ((28, 112), 380.0),
    "tight": ((28, 136), 200.0),
    "sbend": ((62, 118), 300.0),
    "ring": ((66, 142), 300.0),
    "culdesac": ((88, 118), 220.0),
    "gridcurve": ((100, 138), 300.0),
    "avenue": ((120, 114), 340.0),
    "rotgrid": ((150, 146), 380.0),
    "rotstreets": ((155, 115), 380.0),
}


def site(name, mode="top"):
    (cx, cy), d = SITES[name]
    if mode == "top":
        top(cx, cy, d)
    else:
        look(cx, cy, d * 0.8, 0.5, 0.85)


def bed_roads():
    """The road layout as `topdown zonebed` builds it, for reference."""


def roads():
    play.focus()
    play.cmd("money 2000000")
    look(40, 128, 500)
    # Trunk street from the highway stub.
    look(40, 128, 500)
    road("Straight", [(11, 128), (60, 128)])
    look(110, 128, 500)
    road("Straight", [(60, 128), (140, 128)])
    look(170, 128, 500)
    road("Straight", [(140, 128), (185, 128)])
    # Gentle long curve (north).
    site("gentle")
    road("Curve", [(16, 128), (16, 102), (42, 102)])
    # Tight curve (south) near the street's minimum radius, then straight on.
    site("tight")
    road("Curve", [(24, 128), (24, 131), (27, 131)])
    road("Straight", [(27, 131), (38, 131)])
    # S-bend (north).
    site("sbend")
    road("Curve", [(56, 128), (56, 120), (62, 118)])
    road("Curve", [(62, 118), (68, 116), (68, 106)])
    # Ring (south) on a stem.
    site("ring")
    road("Straight", [(66, 128), (66, 134)])
    road("Curve", [(66, 134), (75, 134), (75, 143)])
    road("Curve", [(75, 143), (75, 152), (66, 152)])
    road("Curve", [(66, 152), (57, 152), (57, 143)])
    road("Curve", [(57, 143), (57, 134), (66, 134)])
    # Curved cul-de-sac (north).
    site("culdesac")
    road("Curve", [(86, 128), (86, 118), (95, 114)])
    # A straight grid with a curved street leaving it (south).
    site("gridcurve")
    road("Straight", [(90, 128), (90, 146)])
    road("Straight", [(100, 128), (100, 146)])
    road("Straight", [(90, 137), (100, 137)])
    road("Straight", [(90, 146), (100, 146)])
    road("Curve", [(100, 137), (110, 137), (114, 148)])
    # Curved avenue (north).
    site("avenue")
    road("Curve", [(108, 128), (108, 104), (134, 100)], avenue=True)
    # Rotated grid with the grid shape (south).
    frame([(138, 132), (164, 138), (159, 160)])
    play.road_mode("Grid")
    for p in [(138, 132), (164, 138), (159, 160)]:
        click_at(*p)
    play.key("Return")
    play.wait_frames(3)
    esc()
    road("Straight", [(140, 128), (138, 132)])
    # Rotated streets drawn one by one, blocks not whole cells (north).
    site("rotstreets")
    import math
    o = (146.0, 128.0)
    u = (math.cos(math.radians(-20)), math.sin(math.radians(-20)))
    w = (u[1], -u[0])
    at = lambda a, b: (round(o[0] + u[0] * a + w[0] * b, 2), round(o[1] + u[1] * a + w[1] * b, 2))  # noqa: E731
    for k in range(3):
        road("Straight", [at(0, 6.6 * k), at(29.2, 6.6 * k)])
    for k in range(1, 5):
        road("Straight", [at(7.3 * k, 0), at(7.3 * k, 13.2)])
    road("Straight", [at(0, 0), at(0, 13.2)])
    play.select_tool("inspect")
    play.log(play.cmd("stats"))
    play.log(play.cmd("zonestats"))


def zone():
    """Every zone mode on the inside and the outside of each road."""
    play.focus()
    # Gentle curve: outside along the road, inside with fill.
    site("gentle")
    zone_tool("r")
    zmode("Frontage")
    click_at(14.6, 112)          # outside (west of the curve)
    click_at(30, 104.2)          # outside (north) of the far part
    zmode("Fill")
    click_at(22, 110)            # inside
    # Tight curve: paint the inside, marquee the outside.
    site("tight")
    zone_tool("c")
    zmode("Paint")
    drag([(25.3, 129.2), (25.6, 130), (26.4, 129.6), (28, 129.6), (32, 129.6)])
    zmode("Marquee")
    drag([(21.5, 129), (34, 134)])
    # S-bend: along the road on both sides, dragged.
    site("sbend")
    zone_tool("r")
    zmode("Frontage")
    drag([(57.4, 126), (58, 120.8), (62, 119.4), (66.6, 116.5), (66.6, 108)])
    drag([(54.6, 126), (55.6, 118.5), (62, 116.6), (69.4, 112), (69.4, 108)])
    # Ring: inside with fill, outside along the road.
    site("ring")
    zone_tool("r")
    zmode("Fill")
    click_at(66, 138)
    zmode("Frontage")
    click_at(77, 143)
    click_at(66, 154)
    click_at(55, 143)
    click_at(70, 132.6)
    # Cul-de-sac: marquee over the whole of it.
    site("culdesac")
    zone_tool("r")
    zmode("Marquee")
    drag([(83, 112), (97, 126.5)])
    # Grid with a curve: fill the grid blocks, frontage the curve.
    site("gridcurve")
    zone_tool("c")
    zmode("Fill")
    click_at(95, 132)
    click_at(95, 141)
    zmode("Frontage")
    drag([(103, 135.6), (109, 136), (112.5, 142), (113, 147)])
    drag([(103, 138.6), (108, 139), (110.6, 143), (111.6, 147)])
    # Curved avenue: industry inside with fill, shops outside along it.
    site("avenue")
    zone_tool("i")
    zmode("Fill")
    click_at(116, 115)
    zone_tool("c")
    zmode("Frontage")
    click_at(105.5, 114)
    click_at(122, 101.2)
    # Rotated grid: marquee and fill.
    site("rotgrid")
    zone_tool("r")
    zmode("Fill")
    for p in [(146, 140), (153, 142), (144, 150), (151, 153)]:
        click_at(*p)
    site("rotstreets")
    zone_tool("r")
    import math
    o = (146.0, 128.0)
    u = (math.cos(math.radians(-30)), math.sin(math.radians(-30)))
    v = (u[1], -u[0])
    g = lambda a, b: (o[0] + u[0] * a + v[0] * b, o[1] + u[1] * a + v[1] * b)  # noqa: E731
    zmode("Marquee")
    drag([g(1.0, 1.0), g(7.0, 6.0), g(13.5, 12.0)])
    zmode("Fill")
    for k in range(1, 4):
        for b in (3.3, 9.9):
            click_at(*g(3.6 + 7.3 * k, b))
    play.select_tool("inspect")
    play.log(play.cmd("zonestats"))


def held(path, shot_path):
    """Press at the first point, move through the rest, take a picture
    while the button is still held, then release."""
    move(*path[0])
    play.press()
    for p in path[1:]:
        move(*p)
    play.wait_frames(3)
    play.shot(shot_path)
    play.release()


def depth(label):
    """Pick the zone depth by clicking its button in the drawer."""
    x, y = play.cmd(f"node Depth {label}").split()[:2]
    play.move_px(float(x), float(y))
    play.wait_frames()
    play.click()


def modes(out, n0):
    """Each zone mode's preview on a curve, pictured mid-action."""
    play.focus()
    k = n0
    # Along the road, dragged round the S-bend (both bends) on its east side.
    site("sbend")
    zone_tool("r")
    zmode("Frontage")
    held([(57.4, 126), (57.6, 122), (60, 119.8), (64, 118.4), (66.8, 114), (66.9, 108)],
         os.path.join(out, f"zone-{k:02d}-mode-along-road-sbend.png"))
    k += 1
    # Fill: hovering the bare middle of the ring previews its whole inside.
    site("ring")
    zone_tool("c")
    zmode("Fill")
    move(66, 143)
    play.wait_frames(3)
    play.shot(os.path.join(out, f"zone-{k:02d}-mode-fill-ring-middle.png"))
    k += 1
    # Rectangle across the cul-de-sac's strips and the trunk's grid.
    site("culdesac")
    zone_tool("i")
    zmode("Marquee")
    held([(84.5, 126.5), (90, 120), (93, 116)], os.path.join(out, f"zone-{k:02d}-mode-rectangle-culdesac.png"))
    k += 1
    # Paint across the gentle curve's strip and the trunk's grid where they meet.
    site("gentle")
    zone_tool("c")
    zmode("Paint")
    held([(18.5, 112), (18.2, 118), (18.4, 124), (21, 126.4), (26, 126.5)],
         os.path.join(out, f"zone-{k:02d}-mode-paint-gentle.png"))
    k += 1
    # De-zoning along the road, dragged.
    site("sbend")
    zone_tool("none")
    zmode("Frontage")
    held([(54.6, 126), (55.4, 120), (60, 116.8)], os.path.join(out, f"zone-{k:02d}-mode-dezone-along-road.png"))
    k += 1
    play.select_tool("inspect")
    return k


def neighbourhood():
    """A curvy neighbourhood zoned in a few actions: ring and S-bend homes,
    shops and industry on the other curves for jobs."""
    play.focus()
    site("ring")
    zone_tool("r")
    zmode("Fill")
    click_at(66, 143)                 # the whole inside of the ring
    zmode("Frontage")
    click_at(77, 143)                 # all round its outside
    site("sbend")
    zmode("Frontage")
    drag([(57.4, 126), (57.6, 122), (60, 119.8), (64, 118.4), (66.8, 114), (66.9, 108)])
    drag([(54.6, 126), (55.4, 120), (61, 116.4), (69.3, 112), (69.4, 108)])
    site("gentle")
    zone_tool("c")
    zmode("Fill")
    click_at(22, 110)
    zone_tool("i")
    zmode("Frontage")
    click_at(14.6, 112)
    site("avenue")
    zone_tool("i")
    zmode("Fill")
    click_at(116, 115)
    zone_tool("c")
    zmode("Frontage")
    click_at(105.5, 114)
    site("culdesac")
    zone_tool("c")
    zmode("Marquee")
    drag([(83, 112), (97, 126.5)])
    play.select_tool("inspect")
    play.log(play.cmd("zonestats"))


def shots(prefix, out, n0, suffix):
    """Top-down (zone overlay on) and street-level pictures of every site."""
    k = n0
    play.cmd("camera off")
    for name in SITES:
        zone_tool("r")
        site(name, "top")
        play.wait_frames(4)
        play.shot(os.path.join(out, f"{prefix}-{k:02d}-{suffix}-{name}-zones.png"))
        k += 1
    play.select_tool("inspect")
    for name in SITES:
        site(name, "low")
        play.wait_frames(4)
        play.shot(os.path.join(out, f"{prefix}-{k:02d}-{suffix}-{name}-street.png"))
        k += 1
    return k


def launch(exe, size="1920x1080"):
    os.makedirs(play.CTL, exist_ok=True)
    for f in ("cmd", "reply", "seq"):
        try:
            os.remove(os.path.join(play.CTL, f))
        except OSError:
            pass
    import subprocess
    env = dict(os.environ, DISPLAY=play.DISPLAY, CITYSIM_CONTROL=play.CTL, CITYSIM_GFX_QUALITY="high",
               CITYSIM_GFX_HOUR=os.environ.get("ZPLAY_HOUR", "11"),
               CITYSIM_CONFIG_DIR=os.path.join(play.CTL, "cfg"), CITYSIM_SAVE_DIR=os.path.join(play.CTL, "save"))
    for k in ("WAYLAND_DISPLAY", "XDG_SESSION_TYPE"):
        env.pop(k, None)
    logf = open(os.path.join(play.CTL, "game.log"), "w")
    p = subprocess.Popen([exe], cwd=play.ROOT, env=env, stdout=logf, stderr=subprocess.STDOUT,
                         stdin=subprocess.DEVNULL, start_new_session=True)
    play.log(f"launched {exe} pid {p.pid}")
    for _ in range(1200):
        try:
            play.cmd("fps", timeout=1.0)
            break
        except (TimeoutError, FileNotFoundError, RuntimeError):
            if p.poll() is not None:
                raise RuntimeError("game exited during start")
    for _ in range(1200):
        try:
            play.cmd("ready", timeout=2.0)
            break
        except RuntimeError:
            time.sleep(0.2)
    w, h = size.split("x")
    play.cmd(f"window {w} {h}")
    play.log("up:", play.cmd("stats"))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--ctl", default="/tmp/ctl")
    ap.add_argument("--display", default=":0")
    ap.add_argument("phase")
    ap.add_argument("args", nargs="*")
    a = ap.parse_args()
    play.CTL, play.DISPLAY = a.ctl, a.display
    if a.phase == "launch":
        launch(a.args[0])
        if len(a.args) > 1 and a.args[1] == "load":
            play.focus()
            play.key("F9")
            play.wait_frames(6)
            play.log(play.cmd("stats"))
    elif a.phase == "roads":
        roads()
    elif a.phase == "zone":
        zone()
    elif a.phase == "modes":
        print(modes(a.args[0], int(a.args[1])))
    elif a.phase == "hood":
        neighbourhood()
    elif a.phase == "grow":
        play.cmd("speed") if False else None
        print(play.cmd(f"until-day {a.args[0]} 60", timeout=3600))
    elif a.phase == "shots":
        shots(a.args[0], a.args[1], int(a.args[2]), a.args[3])
    elif a.phase == "site":
        site(a.args[0], a.args[1] if len(a.args) > 1 else "top")
    elif a.phase == "stats":
        print(play.cmd("stats"))
        print(play.cmd("zonestats"))
    else:
        sys.exit("unknown phase")


if __name__ == "__main__":
    main()
