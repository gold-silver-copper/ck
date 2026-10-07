#!/usr/bin/env python3
"""Capture a Wayland output to a video at a steady frame rate, frame by frame with grim
(screencopy works on headless outputs, which screen recorders that read the GPU's
displays don't see).

    capture.py OUTPUT FPS VIDEO START_FILE

Runs until SIGTERM or SIGINT. A frame that takes longer than its slot is repeated, so the
video keeps real time. The time of the first frame goes to START_FILE.
"""

import signal
import subprocess
import sys
import time


def main():
    output, fps, video, start_file = sys.argv[1], int(sys.argv[2]), sys.argv[3], sys.argv[4]
    stop = False

    def stopping(*_):
        nonlocal stop
        stop = True

    signal.signal(signal.SIGTERM, stopping)
    signal.signal(signal.SIGINT, stopping)
    # Lossless, and fast enough to keep up: the final encode is done afterwards.
    ff = subprocess.Popen(
        ["ffmpeg", "-loglevel", "error", "-y", "-f", "image2pipe", "-framerate", str(fps), "-c:v", "ppm", "-i", "-",
         "-c:v", "libx264rgb", "-preset", "ultrafast", "-qp", "0", video],
        stdin=subprocess.PIPE,
    )
    start, n, last = time.time(), 0, None
    with open(start_file, "w") as f:
        f.write(f"{start:.3f}\n")
    while not stop:
        frame = subprocess.run(["grim", "-o", output, "-t", "ppm", "-"], capture_output=True).stdout
        if frame:
            last = frame
        if last is None:
            time.sleep(0.01)
            continue
        # This frame fills every slot up to now.
        due = int((time.time() - start) * fps) + 1
        while n < due:
            ff.stdin.write(last)
            n += 1
        wait = start + n / fps - time.time()
        if wait > 0:
            time.sleep(wait)
    ff.stdin.close()
    ff.wait()


if __name__ == "__main__":
    main()
