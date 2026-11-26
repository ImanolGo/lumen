#!/bin/sh
# Takes one screenshot of a demo scene on an invisible X display (needs Xvfb and ImageMagick).
# usage: SCRATCH=/some/dir tools/screenshots/shoot.sh SCENE OUTNAME THEME WAIT_SECONDS FILE_OR_FOLDER
# Needs a release build with the demo scenes compiled in (see README.md here).
S=${SCRATCH:-/tmp/lumen-shots}
export APP=${APP:-$(dirname "$0")/../../target/release/lumen}
if [ "$1" != --inside ]; then
    exec xvfb-run -a -s "-screen 0 1200x800x24" "$0" --inside "$@"
fi
scene=$2; out=$3; theme=$4; wait=$5; target=$6
W=$(mktemp -d)
mkdir -p "$S/shots" "$W/state/lumen"
echo 'window = [1200, 800]' > "$W/state/lumen/state.toml"
env -u WAYLAND_DISPLAY XDG_CONFIG_HOME="$W/config" XDG_DATA_HOME="$W/data" XDG_STATE_HOME="$W/state" \
    XDG_CACHE_HOME="$W/cache" LUMEN_NO_PORTAL=1 LUMEN_NO_REEXEC=1 WGPU_POWER_PREF=low \
    LANG=en_US.UTF-8 LC_ALL= LANGUAGE= LUMEN_THEME="$theme" LUMEN_DEMO="$scene" "$APP" "$target" >/dev/null 2>&1 &
app=$!
sleep "$wait"
import -window root "$S/shots/$out.png"
kill $app
rm -rf "$W"
