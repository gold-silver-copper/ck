#!/usr/bin/env python3
"""Captions for the recording, from the tour's log: what's being shown on the left of a
band under the terminal, the keys being pressed on the right.

    captions.py LOG START_FILE BAND_COLOR > captions.ass

Waits on the network (the tour's `hold` to `go`) are cut down to their first moment.
Prints to stderr the trim, `FROM TO` (seconds into the capture), then the stretches of it
to keep, one `START END` a line, in seconds from FROM; the captions' times are on the cut
video's clock.
"""

import sys

KEYS = {
    "enter": "⏎", "esc": "esc", "tab": "tab", "btab": "shift-tab", "up": "↑", "down": "↓", "left": "←",
    "right": "→", "space": "space", "bs": "⌫", "ctrl-w": "ctrl-w", "ctrl-d": "ctrl-d", "ctrl-u": "ctrl-u",
}
# Keys pressed this close together show together.
GROUP = 1.2
# How long the last of them stays.
LINGER = 1.4
# How much of a wait on the network is kept: enough to see it start.
KEEP_WAIT = 0.25


def ts(secs):
    secs = max(secs, 0)
    h, rest = divmod(secs, 3600)
    m, s = divmod(rest, 60)
    return f"{int(h)}:{int(m):02}:{s:05.2f}"


def ass_color(rgb):
    # ASS is &HAABBGGRR.
    return f"&H00{rgb[4:6]}{rgb[2:4]}{rgb[0:2]}".upper()


def main():
    log, start_file, band = sys.argv[1], sys.argv[2], sys.argv[3].lstrip("#")
    start = float(open(start_file).read())
    events = []
    for line in open(log, encoding="utf-8"):
        t, kind, text = line.rstrip("\n").split("\t", 2)
        events.append((float(t) - start, kind, text))
    first = next(t for t, kind, _ in events if kind == "scene") - 0.3
    last = next((t for t, kind, _ in events if kind == "end"), events[-1][0])
    # The waits to cut, from just after each starts to when what it waited for came (the
    # tour logs `go` 0.4 s after that, for it to be drawn).
    cuts, held = [], None
    for t, kind, _ in events:
        if kind == "hold":
            held = t
        elif kind == "go" and held is not None:
            a, b = held + KEEP_WAIT, t - 0.4
            if b - a > 0.2 and a >= first:
                cuts.append((a, b))
            held = None
    keep, at = [], first
    for a, b in cuts:
        keep.append((at, a))
        at = b
    keep.append((at, last))

    def clock(t):
        """Where capture time `t` is in the cut video."""
        out = 0.0
        for a, b in keep:
            if t <= a:
                break
            out += min(t, b) - a
        return out

    print(f"{first:.3f} {last:.3f}", file=sys.stderr)
    for a, b in keep:
        print(f"{a - first:.3f} {b - first:.3f}", file=sys.stderr)
    scenes = [(clock(t), text) for t, kind, text in events if kind == "scene"]
    keys = [(clock(t), kind, text) for t, kind, text in events if kind in ("key", "type")]
    first, last = 0.0, clock(last)

    out = [
        "[Script Info]",
        "ScriptType: v4.00+",
        "PlayResX: 1920",
        "PlayResY: 1080",
        "WrapStyle: 2",
        "",
        "[V4+ Styles]",
        "Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, "
        "Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding",
        f"Style: Scene,JetBrainsMono Nerd Font,32,{ass_color('e6e1f5')},&H000000FF,{ass_color(band)},{ass_color(band)},0,0,0,0,100,100,0,0,1,0,0,1,48,48,20,1",
        f"Style: Key,JetBrainsMono Nerd Font,30,{ass_color(band)},&H000000FF,{ass_color('c9b8ff')},{ass_color('c9b8ff')},1,0,0,0,100,100,0,0,3,7,0,3,48,48,22,1",
        "",
        "[Events]",
        "Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text",
    ]
    for i, (t, text) in enumerate(scenes):
        until = scenes[i + 1][0] if i + 1 < len(scenes) else last
        out.append(f"Dialogue: 0,{ts(t - first)},{ts(until - first)},Scene,,0,0,0,,{text}")
    # Runs of keys pressed close together, each shown until the run's last key has lingered.
    runs, run = [], []
    for k in keys:
        if run and k[0] - run[-1][0] > GROUP:
            runs.append(run)
            run = []
        run.append(k)
    if run:
        runs.append(run)
    for i, run in enumerate(runs):
        nxt = runs[i + 1][0][0] if i + 1 < len(runs) else last
        shown = []
        for j, (t, kind, text) in enumerate(run):
            shown.append(text if kind == "type" else KEYS.get(text, text))
            until = run[j + 1][0] if j + 1 < len(run) else min(t + LINGER, nxt)
            label = "  ".join(shown[-8:])
            out.append(f"Dialogue: 1,{ts(t - first)},{ts(until - first)},Key,,0,0,0,,{label}")
    print("\n".join(out))


if __name__ == "__main__":
    main()
