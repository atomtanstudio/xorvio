#!/bin/zsh
# A throwaway Plex Media Server in Docker for testing Xorvio's Plex source:
# two films and a two-season show made with ffmpeg, mounted read-only, and
# the libraries created through Plex's API. The server is unclaimed and lets
# any address in without a token (ALLOWED_NETWORKS), so nothing here signs in
# to plex.tv.
#
#   Scripts/test-servers/plex.sh start   # media, server, libraries (fresh each time)
#   Scripts/test-servers/plex.sh lock    # wants a token from then on, like a claimed server
#   Scripts/test-servers/plex.sh stop    # removes the container
#
# The server answers at http://127.0.0.1:32401. The media is kept in
# target/test-servers/plex-media so later starts don't remake it. Then:
#
#   XORVIO_PLEX_TEST_URL=http://127.0.0.1:32401 cargo test -p xorvio-core --test plex_docker -- --ignored
#
# and, once locked:
#
#   XORVIO_PLEX_LOCKED_TEST_URL=http://127.0.0.1:32401 cargo test -p xorvio-core --test plex_docker -- --ignored
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
NAME="${XORVIO_PLEX_CONTAINER:-xorvio-test-plex}"
PORT="${XORVIO_PLEX_PORT:-32401}"
IMAGE="${XORVIO_PLEX_IMAGE:-plexinc/pms-docker:latest}"
MEDIA="${XORVIO_PLEX_MEDIA:-$ROOT/target/test-servers/plex-media}"
BASE="http://127.0.0.1:$PORT"

say() { print -r -- "plex.sh: $*" >&2; }

# MARK: Media

# One test video: `clip <file> <seconds> <size> <video codec args> <audio args>`.
# They run for minutes, not seconds: Plex ignores ("premature") positions
# in the first minute or so of a title, so a short clip can't be resumed.
clip() {
  local file="$1" seconds="$2" size="$3" video="$4" audio="$5"
  [[ -s "$file" ]] && return 0
  mkdir -p "$(dirname "$file")"
  say "making ${file#$MEDIA/}"
  ffmpeg -nostdin -loglevel error -y \
    -f lavfi -i "testsrc=size=${size}:rate=24:duration=${seconds}" \
    -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=${seconds}" \
    ${=video} ${=audio} -shortest "$file"
}

# A still image for local artwork: `still <file> <size> <colour>`.
still() {
  local file="$1" size="$2" colour="$3"
  [[ -s "$file" ]] && return 0
  mkdir -p "$(dirname "$file")"
  ffmpeg -nostdin -loglevel error -y -f lavfi -i "color=c=${colour}:size=${size}:duration=1" \
    -vf "drawgrid=w=64:h=64:t=2:c=white@0.4" -frames:v 1 "$file"
}

make_media() {
  command -v ffmpeg >/dev/null || { say "ffmpeg is needed (brew install ffmpeg)"; exit 1; }
  local h264="-c:v libx264 -preset ultrafast -crf 30 -pix_fmt yuv420p"
  # 10-bit HEVC tagged as HDR10, so the server reports HDR.
  local hevc="-vf setparams=color_primaries=bt2020:color_trc=smpte2084:colorspace=bt2020nc,format=yuv420p10le
    -c:v libx265 -preset ultrafast -crf 34 -tag:v hvc1
    -x265-params log-level=error:hdr10=1:colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc:max-cll=1000,400"
  local aac="-c:a aac -b:a 96k -ac 2"
  local ac3="-c:a ac3 -b:a 384k -ac 6"

  local one="$MEDIA/Movies/Test Film One (2020)"
  clip "$one/Test Film One (2020).mkv" 360 1280x720 "$h264" "$aac"
  still "$one/poster.jpg" 600x900 navy
  still "$one/fanart.jpg" 1920x1080 teal

  local two="$MEDIA/Movies/Test Film Two (2021)"
  clip "$two/Test Film Two (2021).mkv" 360 1920x1080 "$hevc" "$ac3"
  still "$two/poster.jpg" 600x900 maroon

  local show="$MEDIA/TV Shows/Test Show (2019)"
  still "$show/poster.jpg" 600x900 darkgreen
  still "$show/fanart.jpg" 1920x1080 olive
  local season episode file
  for season in 1 2; do
    for episode in 1 2; do
      file="$(printf '%s/Season %02d/Test Show - s%02de%02d.mkv' "$show" "$season" "$season" "$episode")"
      clip "$file" 180 640x360 "$h264" "$aac"
    done
  done
}

# MARK: Server

api() {
  # (not "path": zsh ties that to PATH)
  local method="$1" target="$2"
  curl -gfsS -X "$method" -H 'Accept: application/json' \
    -H 'X-Plex-Product: Xorvio Test Script' -H 'X-Plex-Client-Identifier: xorvio-test-script' \
    "$BASE$target"
}

# Until /identity answers without a startState: the server takes requests
# while it's still starting its plugins, but turns libraries down until then.
wait_for_server() {
  say "waiting for the server at $BASE"
  local identity
  for _ in {1..120}; do
    identity="$(curl -fsS -m 2 -H 'Accept: application/json' "$BASE/identity" 2>/dev/null || true)"
    if [[ -n "$identity" && "$identity" != *startState* ]]; then return 0; fi
    sleep 1
  done
  say "the server didn't answer; docker logs $NAME may say why"
  exit 1
}

# The id of the library named $1, if there is one.
section_id() {
  api GET /library/sections | python3 -c '
import json, sys
name = sys.argv[1]
for section in json.load(sys.stdin)["MediaContainer"].get("Directory", []):
    if section["title"] == name:
        print(section["key"])
' "$1"
}

# Items of a type (1 film, 2 show, 4 episode) in a library.
count_items() {
  api GET "/library/sections/$1/all?type=$2" | python3 -c '
import json, sys
print(json.load(sys.stdin)["MediaContainer"].get("size", 0))'
}

encode() { python3 -c 'import sys, urllib.parse; print(urllib.parse.quote(sys.argv[1], safe=""))' "$1"; }

add_library() {
  local name="$1" kind="$2" scanner="$3" agent="$4" location="$5"
  [[ -n "$(section_id "$name")" ]] && return 0
  say "adding the $name library"
  api POST "/library/sections?name=$(encode "$name")&type=$kind&agent=$agent&scanner=$(encode "$scanner")&language=xn&location=$(encode "$location")" >/dev/null
}

wait_for_scan() {
  local section="$1" kind="$2" wanted="$3"
  for _ in {1..120}; do
    [[ "$(count_items "$section" "$kind")" -ge "$wanted" ]] && return 0
    sleep 1
  done
  say "the scan didn't find $wanted items of type $kind in library $section"
  exit 1
}

# The rating key of the title called $2 (type $3) in library $1.
rating_key() {
  api GET "/library/sections/$1/all?type=$3" | python3 -c '
import json, sys
for item in json.load(sys.stdin)["MediaContainer"].get("Metadata", []):
    if item["title"].startswith(sys.argv[1]):
        print(item["ratingKey"])
' "$2"
}

# Fills in details the Personal Media agent leaves empty, through Plex's
# own edit endpoint, so Xorvio's details have something to show.
describe() {
  local section="$1" kind="$2" key="$3"; shift 3
  local query="type=$kind&id=$key"
  for pair in "$@"; do
    query="$query&${pair%%=*}=$(encode "${pair#*=}")"
  done
  api PUT "/library/sections/$section/all?$query" >/dev/null
}

start() {
  make_media
  docker rm -f "$NAME" >/dev/null 2>&1 || true
  say "starting $NAME on port $PORT"
  docker run -d --name "$NAME" --hostname xorvio-test-plex \
    -p "$PORT:32400" \
    -e TZ=UTC -e ALLOWED_NETWORKS=0.0.0.0/0 \
    -v "$MEDIA:/data:ro" \
    "$IMAGE" >/dev/null
  wait_for_server
  api PUT "/:/prefs?FriendlyName=$(encode 'Xorvio Test Plex')" >/dev/null

  # The Personal Media agent reads only what's on disk (and the posters and
  # backgrounds beside the files), so nothing is looked up online.
  add_library "Films" movie "Plex Movie" tv.plex.agents.none /data/Movies
  add_library "Shows" show "Plex TV Series" tv.plex.agents.none "/data/TV Shows"
  local films shows
  films="$(section_id Films)"
  shows="$(section_id Shows)"
  wait_for_scan "$films" 1 2
  wait_for_scan "$shows" 4 4

  local one two show
  one="$(rating_key "$films" 'Test Film One' 1)"
  two="$(rating_key "$films" 'Test Film Two' 1)"
  show="$(rating_key "$shows" 'Test Show' 2)"
  describe "$films" 1 "$one" \
    'title.value=Test Film One' 'year.value=2020' \
    'summary.value=A test pattern drifts across a field of colour.' \
    'tagline.value=Every pixel in its place.' \
    'studio.value=Xorvio Test Pictures' 'contentRating.value=PG' \
    'originallyAvailableAt.value=2020-05-01' 'audienceRating.value=7.4' \
    'genre[0].tag.tag=Drama' 'genre[1].tag.tag=Science Fiction' \
    'director[0].tag.tag=Ada Test' 'writer[0].tag.tag=Bea Script' \
    'actor[0].tag.tag=Cam Lens' 'actor[0].tagging.text=The Pattern' \
    'actor[1].tag.tag=Dee Frame' 'actor[1].tagging.text=The Tone' \
    'country[0].tag.tag=Ireland' 'originalTitle.value=Film Un'
  describe "$films" 1 "$two" \
    'title.value=Test Film Two' 'year.value=2021' \
    'summary.value=The same pattern, in HEVC with 5.1 sound.' \
    'genre[0].tag.tag=Documentary' 'contentRating.value=12' 'audienceRating.value=6.1'
  describe "$shows" 2 "$show" \
    'title.value=Test Show' 'year.value=2019' \
    'summary.value=Four short episodes of test patterns.' \
    'studio.value=Xorvio Test Network' 'contentRating.value=TV-PG' \
    'originallyAvailableAt.value=2019-02-03' 'genre[0].tag.tag=Comedy' \
    'actor[0].tag.tag=Eve Signal' 'actor[0].tagging.text=Herself'
  say "ready: $BASE (films in library $films, shows in library $shows)"
}

# Lets no address in without a token, as a claimed server does. There's no
# token to undo it with, so `start` again makes a fresh, open server.
lock() {
  api PUT "/:/prefs?allowedNetworks=$(encode 10.255.255.0/255.255.255.0)" >/dev/null
  say "locked: $BASE now wants a token for everything but /identity (start again to undo)"
}

case "${1:-start}" in
  start) start ;;
  stop) docker rm -f "$NAME" >/dev/null 2>&1 && say "removed $NAME" || say "$NAME wasn't running" ;;
  lock) lock ;;
  media) make_media ;;
  *) say "usage: $0 start|stop|lock|media"; exit 2 ;;
esac
