#!/usr/bin/env python3
"""Roads through grown buildings, played with real input (xdotool) in the
running game (`scripts/play.py launch --demo organic` first). Points are sim
metres; `cargo run -p citysim_sim --example clearprobe -- SAVE` finds lines
through houses in a saved city.

    python3 scripts/cplay.py view X Y [YAW PITCH DIST]   # camera on a point
    python3 scripts/cplay.py draw X1 Y1 X2 Y2 [...]       # straight: click points, no build
    python3 scripts/cplay.py curve X1 Y1 XC YC X2 Y2      # curve mode: start, corner, end
    python3 scripts/cplay.py upgrade X Y                  # upgrade mode, avenue, hover
    python3 scripts/cplay.py build                        # Enter (or click for upgrade)
    python3 scripts/cplay.py cancel                       # Esc
    python3 scripts/cplay.py undo                         # Ctrl+Z
    python3 scripts/cplay.py shot PATH

Each phase prints the stats line's money, buildings and population.
"""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import play  # noqa: E402

# Seconds per input step (CPLAY_FRAME): slower on a software renderer.
play.FRAME = float(os.environ.get("CPLAY_FRAME", play.FRAME))


def px(x, y):
    sx, sy, where = play.cmd(f"screenm {x} {y}").split()
    if where != "inside":
        raise RuntimeError(f"({x}, {y}) is off screen")
    return float(sx), float(sy)


def move(x, y):
    play.move_px(*px(x, y))
    play.wait_frames(2)


def summary():
    s = play.stats()
    print(f"money={s['money']} buildings={s['buildings']} pop={s['pop']} segments={s['segments']} "
          f"tool={s['tool']} road_mode={s['road_mode']} road_kind={s['road_kind']}", flush=True)


def kind(avenue):
    if (play.stats()["road_kind"] == "Avenue") != avenue:
        play.key("r")


def main():
    a = sys.argv[1:]
    ph, nums = a[0], [float(v) for v in a[1:] if v.replace(".", "").replace("-", "").isdigit()]
    play.focus()
    if ph == "view":
        yaw, pitch, dist = (nums[2:] + [0.0, 1.0, 180.0][len(nums[2:]):])[:3]
        play.cmd(f"camera {nums[0]} {nums[1]} {yaw} {pitch} {dist}")
        play.wait_frames(3)
    elif ph in ("draw", "curve"):
        play.road_mode("Straight" if ph == "draw" else "Curve")
        kind("avenue" in a)
        pts = list(zip(nums[0::2], nums[1::2]))
        for k, (x, y) in enumerate(pts):
            move(x, y)
            if k < len(pts) - 1 or ph == "draw":
                play.click()
        play.wait_frames(4)
    elif ph == "upgrade":
        play.road_mode("Upgrade")
        kind(True)
        move(nums[0], nums[1])
        play.wait_frames(4)
    elif ph == "build":
        if play.stats()["road_mode"] == "Upgrade":
            play.click()
        else:
            play.key("Return")
        play.wait_frames(6)
    elif ph == "cancel":
        play.key("Escape")
        play.wait_frames(2)
    elif ph == "undo":
        play.key("ctrl+z")
        play.wait_frames(6)
    elif ph == "shot":
        play.shot(a[1])
    summary()


if __name__ == "__main__":
    main()
