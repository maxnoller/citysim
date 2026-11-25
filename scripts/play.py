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


def zone_rect(a, b, zone):
    select_tool({"r": "zone-r", "c": "zone-c", "i": "zone-i", "none": "dezone"}[zone])
    drag_rect(a, b)
    log(f"zone {zone} {a}..{b}")


def bulldoze_rect(a, b):
    select_tool("bulldoze")
    drag_rect(a, b)
    log(f"bulldoze {a}..{b}")


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
