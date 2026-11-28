#!/usr/bin/env python3
"""Roundabout video: real play on the `rings` demo with real OS input.

A busy crossing on the avenue (cell 80, 128; give-way, its side streets
queueing at the morning rush) is made a medium roundabout with the junction
tool (the small size previewed first), the queue clears, and one car is
followed round the ring. Recorded on a slow software renderer with
`CITYSIM_FRAME_STEP=30` (every frame advances 1/30 s of game time) and a
screen capture; `--join` drops the repeated frames and plays the rest at
30 fps.

    python3 scripts/rplay.py launch            # the game, frame-stepped
    python3 scripts/rplay.py record OUT.mp4    # play while capturing
"""

import os
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import play  # noqa: E402
import zplay  # noqa: E402

# Junction tool drawer at 1920x1080 (interface size 100 %).
RING_BUTTONS = {"mini": (719, 903), "small": (769, 903), "medium": (828, 903), "large": (889, 903)}
JUNCTION = (80, 128)


def game_tick():
    for part in play.cmd("stats").split():
        if part.startswith("tick="):
            return int(part[5:])
    return 0


def wait_game(seconds):
    """Wait until `seconds` of game time (at the current speed) have run."""
    speed = 1
    for part in play.cmd("stats").split():
        if part.startswith("speed_idx="):
            speed = [1, 3, 6, 12][int(part[10:])]
    t0 = game_tick()
    goal = t0 + int(seconds * 20 * speed)
    while game_tick() < goal:
        time.sleep(0.5)


def launch():
    play.launch(
        False,
        "medium",
        "rings",
        ["CITYSIM_DEMO_HOUR=7.55", "CITYSIM_SHOT_SIZE=1920x1080", "CITYSIM_FRAME_STEP=30"],
    )
    play.cmd("window 1920 1080")
    time.sleep(3)


def scene():
    play.FRAME = 0.9
    play.focus()
    jx, jy = JUNCTION
    # The crossing at the rush, a side street queueing onto the avenue.
    zplay.look(jx, jy, 120.0, 0.55, 0.85)
    wait_game(7)
    # The traffic map: the junction red.
    play.key("t")
    wait_game(3)
    play.key("t")
    wait_game(1)
    # Junction tool, small ring previewed, then medium.
    play.key("j")
    wait_game(1)
    play.move_px(*RING_BUTTONS["small"])
    play.click()
    zplay.move(jx, jy)
    wait_game(3)
    play.move_px(*RING_BUTTONS["medium"])
    play.click()
    zplay.move(jx + 0.2, jy)
    wait_game(4)
    play.click()
    wait_game(2)
    play.key("Escape")
    play.move_px(1500, 250)
    # The queue clears (at 3x).
    play.key("2")
    wait_game(14)
    play.key("1")
    # Follow one car round the ring.
    zplay.look(jx, jy, 95.0, 0.55, 0.9)
    wait_game(1)
    car = None
    for _ in range(40):
        try:
            car = play.cmd(f"find-ring-car {jx} {jy}")
            break
        except RuntimeError:
            wait_game(0.5)
    if car:
        x, y = (float(v) for v in car.split()[:2])
        play.move_px(x, y)
        play.click()
        wait_game(0.5)
        play.key("f")
        wait_game(14)
    play.key("Escape")
    # Pull back over the town.
    zplay.look(jx - 10, jy - 6, 380.0, 0.5, 1.0)
    wait_game(5)


def record(out):
    raw = out + ".raw.mkv"
    ff = subprocess.Popen(
        ["ffmpeg", "-loglevel", "error", "-y", "-f", "x11grab", "-framerate", "6", "-video_size", "1920x1080",
         "-draw_mouse", "1", "-i", play.DISPLAY + ".0", "-c:v", "libx264", "-preset", "ultrafast", "-crf", "16",
         raw],
        stdin=subprocess.PIPE,
    )
    try:
        scene()
    finally:
        ff.communicate(b"q")
    join(raw, out)


def join(raw, out):
    """Drop repeated frames (the game draws about two a second) and play the
    rest at 30 fps: each was 1/30 s of game time."""
    subprocess.run(
        ["ffmpeg", "-loglevel", "error", "-y", "-i", raw, "-vf",
         "mpdecimate=hi=200:lo=64:frac=0.05,setpts=N/30/TB", "-r", "30", "-c:v", "libx264", "-pix_fmt", "yuv420p",
         "-crf", "20", "-movflags", "+faststart", out],
        check=True,
    )


def main():
    args = sys.argv[1:]
    if args and args[0] == "--display":
        play.DISPLAY = args[1]
        args = args[2:]
    sys.argv = [sys.argv[0]] + args
    phase = sys.argv[1]
    if phase == "launch":
        launch()
    elif phase == "record":
        record(sys.argv[2])
    elif phase == "join":
        join(sys.argv[2], sys.argv[3])


if __name__ == "__main__":
    main()
