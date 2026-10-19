#!/bin/bash
# Take a screenshot of Lumen without a display: it runs on a private Broadway display
# (with headless Chromium as the viewer Broadway draws for) and throwaway settings.
#
#   build-aux/screenshot.sh OUT.png [ACTION...] -- FILE...
#
# Each ACTION is a window action, as `name` or `name=<GVariant>`, applied in order:
#   build-aux/screenshot.sh shot.png markup "markup-tool=<'arrow'>" -- photo.png
#
# Environment: LUMEN_BIN (default target/release/lumen), SCHEME (prefer-light or
# prefer-dark), WIN_W and WIN_H (window size; Broadway's screen is 1024×768).
# Needs gtk4-broadwayd (part of GTK), chromium and gdbus. Run from the repository root.
set -eu

out=$(realpath -m "$1")
shift
actions=()
while [ $# -gt 0 ] && [ "$1" != "--" ]; do
    actions+=("$1")
    shift
done
[ "${1:-}" = "--" ] && shift

bin=${LUMEN_BIN:-target/release/lumen}
work=$(mktemp -d)
pids=()
cleanup() {
    kill "${pids[@]}" 2>/dev/null || true
    # Chromium's helper processes outlive the main one for a moment.
    pkill -f -- "--user-data-dir=$work/chromium" 2>/dev/null || true
    wait 2>/dev/null || true
    sleep 0.3
    rm -rf "$work" 2>/dev/null || true
}
trap cleanup EXIT

export XDG_CONFIG_HOME=$work/config XDG_DATA_HOME=$work/data XDG_CACHE_HOME=$work/cache
mkdir -p "$XDG_DATA_HOME/lumen"
printf 'window_width = %s\nwindow_height = %s\nsidebar_open = true\n' \
    "${WIN_W:-1000}" "${WIN_H:-720}" > "$XDG_DATA_HOME/lumen/state.toml"

display=:${BROADWAY_NUMBER:-7}
port=$((8080 + ${BROADWAY_NUMBER:-7}))
gtk4-broadwayd "$display" >/dev/null 2>&1 & pids+=($!)
sleep 0.5
chromium --headless=new --no-sandbox --disable-gpu --user-data-dir="$work/chromium" \
    "http://127.0.0.1:$port/" >/dev/null 2>&1 & pids+=($!)
sleep 2
GDK_BACKEND=broadway BROADWAY_DISPLAY=$display ADW_DEBUG_COLOR_SCHEME=${SCHEME:-prefer-light} \
    "$bin" "$@" > "$work/lumen.log" 2>&1 & pids+=($!)

object=/io/github/imanolgo/Lumen/window/1
call() {
    gdbus call --session --dest io.github.imanolgo.Lumen --object-path "$object" \
        --method org.gtk.Actions.Activate "$1" "$2" "{}" >/dev/null
}
for _ in $(seq 50); do
    gdbus introspect --session --dest io.github.imanolgo.Lumen --object-path "$object" \
        >/dev/null 2>&1 && break
    sleep 0.2
done
sleep 2
for action in "${actions[@]}"; do
    case "$action" in
        *=*) call "${action%%=*}" "[${action#*=}]" ;;
        *) call "$action" "[]" ;;
    esac
    sleep 0.4
done
sleep 0.5
call screenshot-to "[<'$out'>]"
sleep 0.5
[ -f "$out" ] || { cat "$work/lumen.log" >&2; echo "no screenshot was written" >&2; exit 1; }
echo "wrote $out"
