#!/bin/sh
# Takes one screenshot of a demo scene on niri (needs wl-clipboard and ImageMagick).
# usage: SCRATCH=/some/dir tools/screenshots/shoot.sh SCENE OUTNAME THEME WAIT_SECONDS FILE_OR_FOLDER
# Needs a release build with the demo scenes compiled in (see README.md here).
cd "$(dirname "$0")/../.."
S=${SCRATCH:-/tmp/lumen-shots}
scene=$1; out=$2; theme=$3; wait=$4; target=$5
pkill -x lumen-rs; sleep 1
mkdir -p $S/shots; rm -rf $S/cfg3; mkdir -p $S/cfg3
(env XDG_CONFIG_HOME=$S/cfg3 XDG_DATA_HOME=$S/data3 LANG=en_US.UTF-8 LC_ALL= LANGUAGE= LUMEN_THEME=$theme LUMEN_DEMO=$scene ./target/release/lumen-rs "$target" >/dev/null 2>&1 &)
sleep 1.2
niri msg action set-window-width 1200 >/dev/null 2>&1
niri msg action set-window-height 800 >/dev/null 2>&1
sleep $wait
niri msg action screenshot-window -d false
sleep 0.6
wl-paste -t image/png > $S/shots/$out.png
pkill -x lumen-rs
sleep 1
identify -format "$out: %wx%h\n" $S/shots/$out.png
