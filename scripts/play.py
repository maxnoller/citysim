#!/usr/bin/env python3
"""Scripted play-through of the real game with real OS input (xdotool).

The game must run with the dev control channel enabled
(`CITYSIM_CONTROL=<dir>`, see crates/game/src/dev/control.rs). The channel
only reports where cells are on screen, frames the camera, fast-forwards,
takes screenshots and reads numbers; every road, zone and bulldoze goes
through the real tools via xdotool mouse and key events.

Usage (python3 stdlib only):

    python3 scripts/play.py launch [--release] [--quality low] [--demo NAME]
    python3 scripts/play.py start               # first road, first zones, unpause
    python3 scripts/play.py grow --until-pop 500 [--max-blocks N] [--turbo 30]
    python3 scripts/play.py road X1 Y1 X2 Y2    # one road through the Road tool
    python3 scripts/play.py zone r|c|i|none X1 Y1 X2 Y2
    python3 scripts/play.py bulldoze X1 Y1 X2 Y2
    python3 scripts/play.py curve X1 Y1 X2 Y2 X3 Y3  # start, corner, end (cells)
    python3 scripts/play.py follow                 # click a house, follow a resident
    python3 scripts/play.py checks [--shots DIR]   # awkward-input checks, PASS / FAIL
    python3 scripts/play.py look CX CY [YAW PITCH DIST] | camera-off
    python3 scripts/play.py shot PATH | stats | counts | fps | key NAME...
    python3 scripts/play.py cmd 'until-day 5'   # raw control command
    python3 scripts/play.py quit

Options before the phase: --ctl DIR (default /tmp/ctl), --display :0.
The grid player's progress is kept in <ctl>/play_state.json so `grow` can be
run in stages.
"""

import argparse
import json
import os
import subprocess
import sys
import time

CTL = "/tmp/ctl"
DISPLAY = ":0"
FRAME = 0.35  # seconds per input step: about two frames at 6 fps
VERBOSE = True

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def log(*a):
    if VERBOSE:
        print(time.strftime("%H:%M:%S"), *a, flush=True)


# ------------------------------------------------------------ control channel


def _next_seq():
    path = os.path.join(CTL, "seq")
    try:
        n = int(open(path).read().strip() or 0) + 1
    except (OSError, ValueError):
        n = int(time.time() * 1000) % 1_000_000_000
    with open(path, "w") as f:
        f.write(str(n))
    return n


def cmd(line, timeout=60.0):
    """Send one control command; return the reply text (raise on err)."""
    seq = _next_seq()
    with open(os.path.join(CTL, "cmd"), "a") as f:
        f.write(f"{seq} {line}\n")
    deadline = time.time() + timeout
    reply = os.path.join(CTL, "reply")
    while time.time() < deadline:
        try:
            with open(reply) as f:
                for r in f:
                    parts = r.rstrip("\n").split(" ", 2)
                    if len(parts) >= 2 and parts[0] == str(seq):
                        text = parts[2] if len(parts) > 2 else ""
                        if parts[1] != "ok":
                            raise RuntimeError(f"{line!r}: {text}")
                        return text
        except FileNotFoundError:
            pass
        time.sleep(0.05)
    raise TimeoutError(f"no reply to {line!r} within {timeout}s")


def stats():
    out = {}
    for kv in cmd("stats").split():
        k, _, v = kv.partition("=")
        try:
            out[k] = float(v) if "." in v else int(v)
        except ValueError:
            out[k] = v
    return out


def wait_frames(n=2):
    time.sleep(FRAME * n)


# ------------------------------------------------------------------- xdotool


def xdo(*args):
    env = dict(os.environ, DISPLAY=DISPLAY)
    return subprocess.run(["xdotool", *map(str, args)], env=env, check=True, capture_output=True, text=True).stdout


_wid = None


def window_id():
    global _wid
    if _wid is None:
        ids = xdo("search", "--name", "^CitySim$").split()
        if not ids:
            raise RuntimeError("CitySim window not found")
        _wid = ids[-1]
    return _wid


def focus():
    try:
        xdo("windowactivate", "--sync", window_id())
    except subprocess.CalledProcessError:
        xdo("windowfocus", window_id())


def move_px(x, y):
    """Move the OS pointer to window-relative logical pixel (x, y)."""
    xdo("mousemove", "--window", window_id(), int(x), int(y))


def cell(x, y):
    """Window pixel of cell (x, y)'s centre, or raise if off screen."""
    sx, sy, where = cmd(f"screen {x} {y}").split()
    if where != "inside":
        raise RuntimeError(f"cell ({x},{y}) is off screen at {sx},{sy}")
    return float(sx), float(sy)


def move_to(x, y):
    move_px(*cell(x, y))
    wait_frames()


def press():
    xdo("mousedown", 1)
    wait_frames()


def release():
    xdo("mouseup", 1)
    wait_frames()


def click():
    press()
    release()


def right_click():
    xdo("click", 3)
    wait_frames()


def key(*names):
    for n in names:
        xdo("key", n)
        wait_frames()


def drag_rect(a, b):
    move_to(*a)
    press()
    # Move in two hops so the drag is seen while the button is held.
    mid = ((a[0] + b[0]) // 2, (a[1] + b[1]) // 2)
    move_to(*mid)
    move_to(*b)
    wait_frames()
    release()


def tool():
    return stats()["tool"]


def select_tool(name):
    """name: road, bulldoze, inspect, zone-r, zone-c, zone-i, dezone."""
    keys = {"road": "r", "bulldoze": "b", "inspect": "i"}
    for _ in range(6):
        cur = tool()
        if cur == name:
            return
        key(keys.get(name, "z"))
    raise RuntimeError(f"could not select tool {name} (have {tool()})")


def clear_way(a, b):
    """Like a player: bulldoze buildings standing where the road must go
    (the sim's own BlockedByBuilding answer names each one). Returns False
    if the road is impossible for another reason (logged)."""
    for _ in range(12):
        r = cmd(f"preview-road {a[0]} {a[1]} {b[0]} {b[1]}").split()
        if r[0] == "ok":
            return True
        if r[0] != "blocked":
            log(f"road {a}->{b} impossible: {' '.join(r[1:])}")
            return False
        c = (int(r[1]), int(r[2]))
        log(f"building in the way at {c}: bulldozing it")
        select_tool("bulldoze")
        if cmd(f"screen {c[0]} {c[1]}").split()[2] != "inside":
            look(c[0], c[1])
        move_to(*c)
        click()
        wait_frames(2)
    return False


def hovered():
    """The cell under the pointer, or None (off the map or over the UI)."""
    r = cmd("hover")
    cell, over_ui = r.split(" over_ui=")
    c = cell.removeprefix("cell=")
    return None if c == "none" or over_ui == "true" else tuple(map(int, c.split()))


def frame(a, b):
    """Centre the camera on a..b, far enough out to see both ends."""
    span = max(abs(a[0] - b[0]), abs(a[1] - b[1])) * 8.0
    look((a[0] + b[0]) / 2, (a[1] + b[1]) / 2, 0.0, 1.15, max(260.0, span * 2.2))


def point_at(c, a, b):
    """Move the pointer onto cell c; re-frame if the UI covers it."""
    for attempt in range(2):
        move_to(*c)
        if hovered() == tuple(c):
            return True
        frame(a, b)
    log(f"cell {c} not reachable under the pointer")
    return False


def build_road(a, b):
    if not clear_way(a, b):
        return stats()
    # Frame both ends: the clicks must land on visible cells.
    if any(cmd(f"screen {c[0]} {c[1]}").split()[2] != "inside" for c in (a, b)):
        frame(a, b)
    for _ in range(2):
        select_tool("road")
        if point_at(a, a, b):
            click()
            if point_at(b, a, b):
                click()
        if tool() == "road":
            key("Return")  # build the path (two corners); keeps the Road tool
        if cmd(f"preview-road {a[0]} {a[1]} {b[0]} {b[1]}") == "ok 0":
            break
        log(f"road {a}->{b} not built yet, retrying")
        frame(a, b)
    after = stats()
    log(f"road {a}->{b}: segments {after['segments']} money {after['money']}")
    return after


def set_mode(which, name):
    """Press Tab until the current tool's mode is `name` (which: road_mode,
    zone_mode, bulldoze_mode; names as the control channel prints them)."""
    for _ in range(8):
        if stats()[which] == name:
            return
        key("Tab")
    raise RuntimeError(f"could not set {which} to {name} (have {stats()[which]})")


def zone_rect(a, b, zone):
    select_tool({"r": "zone-r", "c": "zone-c", "i": "zone-i", "none": "dezone"}[zone])
    set_mode("zone_mode", "Marquee")
    drag_rect(a, b)
    log(f"zone {zone} {a}..{b}")


def bulldoze_rect(a, b):
    select_tool("bulldoze")
    set_mode("bulldoze_mode", "Area")
    drag_rect(a, b)
    log(f"bulldoze {a}..{b}")


def road_mode(name):
    select_tool("road")
    set_mode("road_mode", name)


def curve(a, corner, b, avenue=False):
    """A curved road through the Curve tool: three clicks."""
    road_mode("Curve")
    if (stats()["road_kind"] == "Avenue") != avenue:
        key("r")
    before = stats()["segments"]
    for c in (a, corner, b):
        move_to(*c)
        click()
    key("Return")
    wait_frames(2)
    after = stats()["segments"]
    log(f"curve {a} {corner} {b}: segments {before} -> {after}")
    return after - before


def follow_resident():
    """Click a house with Inspect, open a resident, follow them."""
    select_tool("inspect")
    pos = cmd("find-home").split()
    if pos[0] != "ok":
        raise RuntimeError("no home found")
    x, y = int(pos[1]), int(pos[2])
    look(x, y, 0.4, 0.9, 160.0)
    move_to(x, y)
    click()
    wait_frames(4)
    # The first person link of the home's panel, then Follow.
    px_, py_ = map(float, cmd("node Person link").split())
    move_px(px_, py_)
    wait_frames(2)
    click()
    wait_frames(4)
    fx, fy = map(float, cmd("node Follow").split())
    move_px(fx, fy)
    wait_frames(2)
    click()
    wait_frames(2)
    log(f"following: {stats()['following']}")
    return x, y


def look(cx, cy, yaw=0.0, pitch=1.15, dist=260.0):
    cmd(f"look {cx} {cy} {yaw} {pitch} {dist}")
    wait_frames(2)


def shot(path):
    path = os.path.abspath(path)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    cmd(f"shot {path}", timeout=120)
    for _ in range(50):
        if os.path.exists(path) and os.path.getsize(path) > 0:
            break
        time.sleep(0.1)
    log("shot", path)
    return path


# --------------------------------------------------------------- grid player
# Same plan as citysim_sim::scenarios::GridPlayer: 10x5 blocks east of the
# highway, mixed rows of housing / shops / industry, following demand.

COLS = 9
ROWS = {
    "r": [(0, True), (0, False), (2, True), (2, False), (3, True), (5, True), (5, False), (6, True)],
    "c": [(1, False), (4, True), (7, False)],
    "i": [(1, True), (3, False), (4, False), (6, False), (7, True)],
}
ZI = {"r": 0, "c": 1, "i": 2}
ROAD_COST_PER_CELL = 65  # citysim_sim::balance::ROAD_COST_PER_CELL
BLOCK_MAX_COST = 30 * ROAD_COST_PER_CELL


def block_list(rows):
    v = []
    for k, (row, north) in enumerate(rows):
        for j in range(COLS):
            x0 = 20 + 10 * j
            y0 = 128 - 5 * (row + 1) if north else 128 + 5 * row
            band = row // 3
            v.append(((band * 1000 + j * 10 + k, x0, y0), (x0, y0)))
    v.sort()
    return [b for _, b in v]


BLOCKS = {z: block_list(r) for z, r in ROWS.items()}


def state_path():
    return os.path.join(CTL, "play_state.json")


def load_state():
    try:
        return json.load(open(state_path()))
    except (OSError, ValueError):
        return {"next": {"r": 0, "c": 0, "i": 0}, "blocks": 0}


def save_state(st):
    json.dump(st, open(state_path(), "w"))


def build_block(x0, y0, z):
    x1, y1 = x0 + 10, y0 + 5
    look(x0 + 5, y0 + 2.5)
    s = None
    # Unlike the sim's GridPlayer, a sensible player never leaves a block
    # unconnected: extend the avenue and link the block's west edge to it
    # (both lie on future block edges; existing road pieces cost nothing).
    near = y1 if y1 <= 128 else y0
    if x0 > 20 or near != 128:
        look((20 + x0) / 2 + 2, (128 + y0 + 2.5) / 2, 0.0, 1.15, 300.0)
        if x0 > 20:
            build_road((20, 128), (x0, 128))
        if near != 128:
            build_road((x0, near), (x0, 128))
        look(x0 + 5, y0 + 2.5)
    for a, b in [((x0, y0), (x1, y0)), ((x0, y1), (x1, y1)), ((x0, y0), (x0, y1)), ((x1, y0), (x1, y1))]:
        s = build_road(a, b)
    zone_rect((x0 + 1, y0 + 1), (x1 - 1, y1 - 1), z)
    select_tool("inspect")
    return s


def act(st):
    """One decision round (GridPlayer::act). Returns blocks built."""
    s = stats()
    built = 0
    if s["segments"] <= 1:
        look(16, 128)
        build_road((11, 128), (20, 128))
        s = stats()
    for z in ("r", "c", "i"):
        zi = ZI[z]
        free = s[f"free_{z}"]
        d = s[["demand_r", "demand_c", "demand_i"][zi]]
        want = d > 0.1 or (free == 0 and st["next"][z] == 0)
        n = st["next"][z]
        if want and free < 24 and s["money"] > BLOCK_MAX_COST + 1000 and n < len(BLOCKS[z]):
            x0, y0 = BLOCKS[z][n]
            log(f"block {z} #{n} at ({x0},{y0}) demand {d:+.2f} free {free}")
            build_block(x0, y0, z)
            st["next"][z] += 1
            st["blocks"] += 1
            built += 1
            save_state(st)
            s = stats()
    return built


def next_hour(h):
    return (int(h) + 1) % 24


def grow(until_pop, max_blocks, turbo, shots_every=None, shot_dir=None):
    st = load_state()
    cmd("turbo 0")
    try:
        built = 0
        while True:
            s = stats()
            if s["pop"] >= until_pop:
                log(f"reached {s['pop']} residents on day {s['day']}")
                break
            if s["paused"] == "true":
                key("space")
            built += act(st)
            if max_blocks is not None and built >= max_blocks:
                log("max blocks reached")
                break
            # Fast-forward to the next in-game hour (GridPlayer acts hourly);
            # time kept running while the blocks were drawn.
            cmd(f"until-hour {next_hour(stats()['hour'])} {turbo}", timeout=300)
            s = stats()
            log(
                f"day {s['day']} {s['hour']:05.2f}h pop {s['pop']} bldg {s['buildings']} cars {s['cars']} "
                f"money {s['money']} unemp {s['unemployment']:.2f} demand {s['demand_r']:+.2f}/"
                f"{s['demand_c']:+.2f}/{s['demand_i']:+.2f}"
            )
    finally:
        cmd("turbo 0")


# -------------------------------------------------------------- input checks


def _segs():
    s = stats()
    return s["segments"], s["nodes"], s["money"]


def checks(shot_dir):
    """Awkward real-input cases for the road tools on a new game (run right
    after `launch`); prints PASS / FAIL for each."""
    global FRAME
    results = []

    def report(name, ok, detail=""):
        results.append((name, ok))
        log(("PASS " if ok else "FAIL ") + name + (f" ({detail})" if detail else ""))
        if shot_dir:
            shot(os.path.join(shot_dir, name + ".png"))

    def area(cx, cy, mode="Straight"):
        # The focus a little below the action: points from 10 cells north
        # to 5 south of (cx, cy) stay clear of the bars at the bottom.
        look(cx + 4, cy + 3, 0.0, 1.1, 320)
        road_mode(mode)
        return cx, cy

    focus()
    if stats()["paused"] != "true":
        key("space")
    select_tool("road")
    road_mode("Straight")

    # 1. Very fast clicks: press / release with no wait, then Enter.
    cx, cy = area(40, 104)
    seg0 = _segs()[0]
    old = FRAME
    FRAME = 0.02
    move_to(cx - 10, cy)
    click()
    move_to(cx + 4, cy)
    click()
    key("Return")
    FRAME = old
    wait_frames(3)
    report("fast-clicks", _segs()[0] > seg0, f"segments {seg0} -> {_segs()[0]}")

    # 2. Drag off the window: press on the map, leave the window, release
    #    outside; nothing is built, the start point stays, Esc clears it.
    cx, cy = area(62, 104)
    seg0 = _segs()[0]
    move_to(cx - 8, cy)
    press()
    xdo("mousemove", 1700, 400)  # beyond the window's right edge
    wait_frames(2)
    xdo("mousemove", 1900, 1070)
    release()
    wait_frames(2)
    # One Esc drops the start point; a second would leave the tool.
    key("Escape")
    move_to(cx, cy + 4)
    wait_frames(2)
    report("drag-off-window", _segs()[0] == seg0, f"segments {seg0} -> {_segs()[0]}")

    # 3. Press on the interface, release on the map, then Enter: no point
    #    was placed, nothing is built, the tool is still the road tool.
    cx, cy = area(84, 104)
    seg0 = _segs()[0]
    ww, wh = map(float, cmd("window").split())
    move_px(ww / 2, wh - 30)  # the toolbar
    press()
    move_to(cx - 6, cy)
    release()
    move_to(cx + 6, cy)
    key("Return")
    wait_frames(2)
    report("ui-to-map", _segs()[0] == seg0 and stats()["tool"] == "road", f"segments {seg0} -> {_segs()[0]}")

    # 4. Two snap candidates at once: a road from the south-west towards the
    #    dead end of road 1, aimed a little past it: the end snaps to the
    #    dead end, where the node, the 45-degree step and the whole-cell
    #    length agree.
    cx, cy = area(40, 104)
    seg0, nodes0, _ = _segs()
    move_to(cx - 4, cy - 8)
    click()
    move_to(cx + 4, cy)
    click()
    key("Return")
    wait_frames(2)
    seg1, nodes1, _ = _segs()
    # One new node (its own start): the end joined the existing dead end.
    report("two-snaps", seg1 > seg0 and nodes1 == nodes0 + 1, f"segments {seg0} -> {seg1}, nodes {nodes0} -> {nodes1}")

    # 5. A continuous road that loops back over itself: the crossing becomes
    #    a junction and the road is built.
    cx, cy = area(62, 84, "Continuous")
    seg0, nodes0, _ = _segs()
    for c in [(cx - 12, cy + 3), (cx, cy + 3), (cx + 5, cy - 2), (cx, cy - 8), (cx - 5, cy - 3), (cx - 3, cy + 5)]:
        move_to(*c)
        click()
    key("Return")
    wait_frames(3)
    seg1, nodes1, _ = _segs()
    report("self-crossing-loop", seg1 >= seg0 + 3, f"segments {seg0} -> {seg1}, nodes {nodes0} -> {nodes1}")

    # 6. Zoom and rotate mid-draw: the start point stays put in the world.
    cx, cy = area(84, 84)
    seg0 = _segs()[0]
    move_to(cx - 8, cy)
    click()
    cmd("camera off")
    xdo("click", "--repeat", "3", "5")  # wheel out
    xdo("keydown", "q")
    time.sleep(0.25)
    xdo("keyup", "q")
    xdo("click", "--repeat", "2", "4")  # wheel in
    wait_frames(4)
    move_to(cx + 6, cy)
    click()
    key("Return")
    wait_frames(3)
    report("zoom-rotate-mid-draw", _segs()[0] > seg0, f"segments {seg0} -> {_segs()[0]}")

    # 7. Curve tool, three clicks, then undo with Ctrl+Z.
    cx, cy = area(40, 80)
    seg0, _, money0 = _segs()
    made = curve((cx - 6, cy + 4), (cx + 6, cy + 4), (cx + 6, cy - 8))
    after = _segs()[0]
    key("ctrl+z")
    wait_frames(3)
    report("curve-and-undo", made > 0 and _segs()[0] == seg0, f"segments {seg0} -> {after} -> {_segs()[0]}")

    # 8. Backspace and right-click step back without building.
    cx, cy = area(84, 64)
    seg0 = _segs()[0]
    for c in [(cx - 8, cy), (cx, cy), (cx, cy - 8)]:
        move_to(*c)
        click()
    # Two Backspaces leave the start point; a right-click drops it (a
    # second one would leave the tool).
    key("BackSpace", "BackSpace")
    right_click()
    key("Return")
    wait_frames(2)
    report("step-back", _segs()[0] == seg0 and stats()["tool"] == "road", f"segments {seg0} -> {_segs()[0]}")

    failed = [n for n, ok in results if not ok]
    log(f"{len(results) - len(failed)} of {len(results)} checks passed" + (f"; failed: {failed}" if failed else ""))
    return not failed


# ------------------------------------------------------------------- phases


def launch(release, quality, demo, extra_env):
    os.makedirs(CTL, exist_ok=True)
    for f in ("cmd", "reply", "seq", "play_state.json"):
        try:
            os.remove(os.path.join(CTL, f))
        except OSError:
            pass
    profile = "release" if release else "debug"
    exe = os.path.join(ROOT, "target", profile, "citysim")
    env = dict(os.environ, DISPLAY=DISPLAY, CITYSIM_CONTROL=CTL, CITYSIM_GFX_QUALITY=quality)
    if demo:
        env["CITYSIM_DEMO"] = demo
    for kv in extra_env:
        k, _, v = kv.partition("=")
        env[k] = v
    logf = open(os.path.join(CTL, "game.log"), "w")
    p = subprocess.Popen([exe], cwd=ROOT, env=env, stdout=logf, stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL,
                         start_new_session=True)
    log(f"launched pid {p.pid} ({profile}), log {CTL}/game.log")
    for _ in range(600):
        try:
            cmd("fps", timeout=1.0)
            break
        except (TimeoutError, FileNotFoundError):
            if p.poll() is not None:
                raise RuntimeError("game exited during start")
    for _ in range(600):
        try:
            cmd("ready", timeout=2.0)
            break
        except RuntimeError:
            time.sleep(0.2)
    log("control channel up:", cmd("stats"))


def start():
    """Empty map -> first road from the highway stub, first zones, unpause."""
    focus()
    look(18, 128, 0.0, 1.1, 230)
    build_road((11, 128), (30, 128))
    # Leave columns 20 and 30 free: the grid player's streets run there.
    zone_rect((12, 126), (19, 127), "r")
    zone_rect((21, 126), (29, 127), "r")
    zone_rect((12, 129), (19, 130), "c")
    zone_rect((21, 129), (29, 130), "i")
    select_tool("inspect")
    key("space")
    log(cmd("stats"))


def main():
    global CTL, DISPLAY, FRAME
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--ctl", default=CTL)
    ap.add_argument("--display", default=os.environ.get("PLAY_DISPLAY", DISPLAY))
    ap.add_argument("phase")
    ap.add_argument("args", nargs="*")
    ap.add_argument("--release", action="store_true")
    ap.add_argument("--quality", default="low")
    ap.add_argument("--demo")
    ap.add_argument("--env", action="append", default=[], help="extra KEY=VALUE for launch")
    ap.add_argument("--until-pop", type=int, default=500)
    ap.add_argument("--max-blocks", type=int)
    ap.add_argument("--turbo", type=int, default=30)
    ap.add_argument("--shots", help="checks: save a picture after each case here")
    ap.add_argument("--frame", type=float, default=FRAME, help="seconds per input step (default 0.35)")
    a = ap.parse_args()
    CTL, DISPLAY, FRAME = a.ctl, a.display, a.frame
    ints = lambda: [int(x) for x in a.args[-4:]]  # noqa: E731
    if a.phase == "launch":
        launch(a.release, a.quality, a.demo, a.env)
    elif a.phase == "start":
        start()
    elif a.phase == "grow":
        focus()
        grow(a.until_pop, a.max_blocks, a.turbo)
    elif a.phase == "road":
        x1, y1, x2, y2 = ints()
        build_road((x1, y1), (x2, y2))
    elif a.phase == "zone":
        x1, y1, x2, y2 = ints()
        zone_rect((x1, y1), (x2, y2), a.args[0])
    elif a.phase == "bulldoze":
        x1, y1, x2, y2 = ints()
        bulldoze_rect((x1, y1), (x2, y2))
    elif a.phase == "curve":
        v = [int(x) for x in a.args[:6]]
        focus()
        curve((v[0], v[1]), (v[2], v[3]), (v[4], v[5]))
    elif a.phase == "follow":
        focus()
        follow_resident()
    elif a.phase == "checks":
        ok = checks(a.shots)
        sys.exit(0 if ok else 1)
    elif a.phase == "look":
        look(*[float(x) for x in a.args])
    elif a.phase == "camera-off":
        cmd("camera off")
    elif a.phase == "shot":
        shot(a.args[0])
    elif a.phase == "stats":
        print(cmd("stats"))
    elif a.phase in ("counts", "fps"):
        print(cmd(a.phase))
    elif a.phase == "key":
        key(*a.args)
    elif a.phase == "cmd":
        print(cmd(" ".join(a.args), timeout=600))
    elif a.phase == "quit":
        cmd("quit")
    else:
        sys.exit(f"unknown phase {a.phase}")


if __name__ == "__main__":
    main()
