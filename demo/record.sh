#!/usr/bin/env bash
# Record a tour of ck on 4chan's /g/ to a .webm, without touching your screen: ck runs in
# foot on a virtual Hyprland monitor that's never shown, captured frame by frame with grim,
# then captioned (what's shown, and the keys pressed) and encoded. Only the mouse pointer is
# hidden while it captures (Hyprland would draw it in the middle of the virtual monitor),
# and comes back when the recording ends.
#
#   demo/record.sh [out.webm]        (default: ck-demo.webm here, and ck-demo.avif beside it)
#
# Needs Hyprland, foot, grim, ffmpeg (with libvpx-vp9, libsvtav1 and libass) and python3; builds ck
# first. ck gets a scratch config and data directory, so your own sites, watched threads
# and history are neither used nor shown. It browses live /g/: watch the result before
# publishing it.
#
# CK_DEMO_PACE (0.7) scales the tour's pauses (1 is slower); CK_DEMO_SPEED (1.2) speeds up
# the video, captions and all, to keep it under two minutes; CK_DEMO_FPS (20) is the frame
# rate; CK_DEMO_CRF (38) the quality: lower is better and bigger, and 38 keeps the four minutes
# under GitHub's 10 MB for a video in a README. CK_DEMO_SHOTS=1 keeps a screenshot of the
# end of each scene, for checking the tour. The working directory (~/.cache/ck-demo) is
# kept for a look afterwards.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
root=$(dirname "$here")
out=$(realpath -m "${1:-ck-demo.webm}")
work="${XDG_CACHE_HOME:-$HOME/.cache}/ck-demo"
output=CKDEMO
fps=${CK_DEMO_FPS:-20}

for tool in hyprctl foot grim ffmpeg python3 cargo; do
    command -v "$tool" >/dev/null || { echo "ck demo: needs $tool" >&2; exit 1; }
done
[[ -n ${HYPRLAND_INSTANCE_SIGNATURE:-} ]] || { echo "ck demo: needs a running Hyprland session" >&2; exit 1; }
if hyprctl monitors -j | grep -q "\"$output\""; then
    echo "ck demo: the $output output exists already (another recording running?)" >&2
    exit 1
fi

echo "Building ck…"
(cd "$root" && cargo build --release --quiet)
ck="$root/target/release/ck"

rm -rf "$work"
mkdir -p "$work/config/ck" "$work/shots"
# Live /g/ has slurs and the odd lewd thumbnail: ck's own filters keep them out of the
# recording (hidden threads and posts aren't shown, and the tour never asks to see them).
cat > "$work/config/ck/config.toml" <<'EOF'
notify = "off"

[[filter]]
pattern = '(?i)nigg|fag|kike|tranny|troon|retard|jeet|saar|chink|gook|spic\b|goy|\(\(\(|loli|oppai|megamilk|lewd|nsfw|porn|hentai|cock|boob|tits|\bcum|fuck|shit|bing bong|omarchy|remigration|nationalis|israel|jew|zionis|in common|fa/g/|attractive|female'
field = ["subject", "comment", "filename"]
label = "not for a README"

[[filter]]
pattern = '(?i)/(aicg|sdg|ldg|adt|de3|dalle|lmg|ai[a-z]*g)/'
field = "subject"
label = "AI image generals"
EOF

capture=
# The pointer as it was, put back on the way out.
pointer=$(hyprctl getoption cursor:invisible -j | python3 -c 'import json,sys;print(str(json.load(sys.stdin)["bool"]).lower())')
cleanup() {
    hyprctl eval "hl.config({ cursor = { invisible = $pointer } })" >/dev/null 2>&1 || true
    if [[ -n $capture ]]; then
        kill "$capture" 2>/dev/null || true
        wait "$capture" 2>/dev/null || true
    fi
    # The window, if the tour didn't end by itself.
    for pid in $(hyprctl clients -j | python3 -c 'import json,sys;[print(c["pid"]) for c in json.load(sys.stdin) if c["class"]=="ck-demo"]'); do
        kill "$pid" 2>/dev/null || true
    done
    hyprctl output remove "$output" >/dev/null 2>&1 || true
}
trap cleanup EXIT
trap 'exit 130' INT TERM

hyprctl output create headless "$output" >/dev/null
# Far from your monitors, so the pointer can't wander onto it. 1008 high: with the caption
# band under it, the video is 1920x1080.
hyprctl eval "hl.monitor({ output = '$output', mode = '1920x1008@30', position = '20000x0', scale = 1.5 })" >/dev/null
sleep 1
# Opened on the workspace that monitor shows, without focusing it.
ws=$(hyprctl monitors -j | python3 -c "import json,sys;print([m for m in json.load(sys.stdin) if m['name']=='$output'][0]['activeWorkspace']['name'])")

echo "Recording; the tour takes a few minutes, and nothing shows on your screen (the pointer hides until it's done)…"
hyprctl eval "hl.config({ cursor = { invisible = true } })" >/dev/null
python3 "$here/capture.py" "$output" "$fps" "$work/raw.mkv" "$work/start" &
capture=$!
shots=
[[ -n ${CK_DEMO_SHOTS:-} ]] && shots="CK_DEMO_SHOTS=$work/shots CK_DEMO_OUTPUT=$output"
cmd="foot --app-id=ck-demo env XDG_CONFIG_HOME=$work/config XDG_DATA_HOME=$work/data XDG_CACHE_HOME=$work/cache CK_NO_EXTERNAL=1 CK_FRAME_DUMP=$work/frame.txt CK_DEMO_LOG=$work/log CK_DEMO_DONE=$work/done CK_DEMO_PACE=${CK_DEMO_PACE:-0.7} $shots CK_BIN=$ck python3 $here/tour.py"
hyprctl eval "hl.exec_cmd([[$cmd]], { workspace = '$ws silent', fullscreen = true })" >/dev/null

# Until the tour says it's done, or its window is gone (at most 20 minutes).
for _ in $(seq 1200); do
    sleep 1
    grep -q "	end	" "$work/log" 2>/dev/null && break
    if [[ -s $work/log ]] && ! hyprctl clients -j | grep -q '"ck-demo"'; then
        echo "ck demo: the window closed before the tour ended" >&2
        exit 1
    fi
    if grep -q "	scene	" "$work/log" 2>/dev/null; then
        printf '\r  %s' "$(grep "	scene	" "$work/log" | tail -1 | cut -f3)                    "
    fi
done
echo
# Stop recording, and only then let the tour quit ck: its window closing (and the desktop
# behind it) is never captured.
sleep 1
kill "$capture"
wait "$capture" || true
capture=
hyprctl eval "hl.config({ cursor = { invisible = $pointer } })" >/dev/null
touch "$work/done"
for _ in $(seq 50); do
    hyprctl clients -j | grep -q '"ck-demo"' || break
    sleep 0.2
done

echo "Encoding…"
band=$(ffmpeg -loglevel error -ss 2 -i "$work/raw.mkv" -frames:v 1 -vf crop=1:1:3:3 -f rawvideo -pix_fmt rgb24 - | od -An -tx1 | tr -d ' \n')
python3 "$here/captions.py" "$work/log" "$work/start" "$band" > "$work/captions.ass" 2> "$work/trim"
read -r from to < "$work/trim"
# The stretches to keep (waits on the network cut), as one select expression.
keep=$(tail -n +2 "$work/trim" | awk '{printf "%sbetween(t,%s,%s)", (NR > 1 ? "+" : ""), $1, $2}')
ffmpeg -loglevel error -stats -y -ss "$from" -to "$to" -i "$work/raw.mkv" \
    -vf "select='$keep',setpts=N/FRAME_RATE/TB,pad=1920:1080:0:0:color=0x$band,ass=$work/captions.ass,setpts=PTS/${CK_DEMO_SPEED:-1.2},fps=$fps" \
    -c:v libvpx-vp9 -crf "${CK_DEMO_CRF:-38}" -b:v 0 -row-mt 1 -deadline good -cpu-used 2 -pix_fmt yuv420p "$out"
echo "Wrote $out ($(du -h "$out" | cut -f1))"
# The same as an animated AVIF: GitHub shows images in a README, but not videos from the
# repository.
avif="${out%.*}.avif"
SVT_LOG=1 ffmpeg -loglevel error -y -i "$out" -c:v libsvtav1 -crf 42 -preset 6 -pix_fmt yuv420p "$avif"
echo "Wrote $avif ($(du -h "$avif" | cut -f1))"
