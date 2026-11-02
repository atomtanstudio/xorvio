#!/bin/zsh
# Throwaway Jellyfin and Emby servers in Docker, for trying Xorvio's media
# server sources and for the ignored integration test
# (crates/xorvio-core/tests/media_servers_docker.rs).
#
#   Scripts/test-servers/jellyfin-emby.sh          start both, with test media
#   Scripts/test-servers/jellyfin-emby.sh stop     stop and remove both
#
# Each start is fresh: the servers keep nothing between runs (their settings
# live in anonymous volumes removed with the containers). The test media is
# made once with ffmpeg under target/test-servers/media (fictional films and a
# series with their own artwork and .nfo details, so the servers never look
# anything up online) and mounted read-only.
#
# Needs Docker (Colima or Docker Desktop: the media folder is under your home
# folder so either can mount it), ffmpeg, curl and jq. The Emby image is the
# Apple silicon one; on an Intel Mac, run with EMBY_IMAGE=emby/embyserver.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
MEDIA="${XORVIO_TEST_MEDIA:-$ROOT/target/test-servers/media}"
JELLYFIN_IMAGE="${JELLYFIN_IMAGE:-jellyfin/jellyfin:latest}"
EMBY_IMAGE="${EMBY_IMAGE:-emby/embyserver_arm64v8:latest}"
JELLYFIN_PORT=18096
EMBY_PORT=18097
JELLYFIN_CONTAINER=xorvio-test-jellyfin
EMBY_CONTAINER=xorvio-test-emby
# Throwaway credentials for servers that only ever hold test media.
TEST_USER=xorvio
TEST_PASSWORD=xorvio-test-password
CLIENT_HEADER='MediaBrowser Client="Xorvio test setup", Device="script", DeviceId="xorvio-test-setup", Version="1.0"'

stop_servers() {
  docker rm -f -v "$JELLYFIN_CONTAINER" "$EMBY_CONTAINER" >/dev/null 2>&1 || true
}

if [[ "${1:-}" == "stop" ]]; then
  stop_servers
  echo "Stopped and removed $JELLYFIN_CONTAINER and $EMBY_CONTAINER."
  exit 0
fi

# MARK: - Test media

# A still image: a test pattern of the given size and colour.
image() {
  local out="$1" size="$2" colour="$3"
  [[ -f "$out" ]] && return
  ffmpeg -hide_banner -loglevel error -y -f lavfi -i "color=c=${colour}:size=${size}:duration=1" \
    -f lavfi -i "testsrc=size=${size}:duration=1" -filter_complex "[0][1]blend=all_mode=overlay:all_opacity=0.35" \
    -frames:v 1 -update 1 "$out"
}

# A six-minute video (long enough that servers remember where it was left:
# both ignore resume points in anything under five minutes).
video() {
  local out="$1" kind="$2"
  [[ -f "$out" ]] && return
  local tmp="${out:r}.partial.${out:e}"
  case "$kind" in
    h264-aac)
      ffmpeg -hide_banner -loglevel error -y -f lavfi -i "testsrc=size=1280x720:rate=10:duration=360" \
        -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=360" -c:v libx264 -preset ultrafast -crf 32 \
        -pix_fmt yuv420p -c:a aac -b:a 96k -ac 2 -movflags +faststart "$tmp" ;;
    hevc-ac3)
      ffmpeg -hide_banner -loglevel error -y -f lavfi -i "testsrc=size=1920x1080:rate=5:duration=360" \
        -f lavfi -i "anullsrc=channel_layout=5.1:sample_rate=48000" -t 360 -c:v libx265 -preset ultrafast -crf 36 \
        -x265-params log-level=error -tag:v hvc1 -c:a ac3 -b:a 192k "$tmp" ;;
    episode)
      ffmpeg -hide_banner -loglevel error -y -f lavfi -i "testsrc=size=640x360:rate=10:duration=360" \
        -f lavfi -i "sine=frequency=330:sample_rate=48000:duration=360" -c:v libx264 -preset ultrafast -crf 34 \
        -pix_fmt yuv420p -c:a aac -b:a 64k -ac 2 "$tmp" ;;
  esac
  mv "$tmp" "$out"
}

make_media() {
  local film_one="$MEDIA/Movies/Test Film One (2020)"
  local film_two="$MEDIA/Movies/Test Film Two (2021)"
  local show="$MEDIA/Shows/Test Show (2019)"
  mkdir -p "$film_one" "$film_two" "$show/Season 01" "$show/Season 02"

  video "$film_one/Test Film One (2020).mp4" h264-aac
  image "$film_one/poster.jpg" 600x900 navy
  image "$film_one/fanart.jpg" 1920x1080 teal
  image "$film_one/logo.png" 800x310 white
  cat > "$film_one/Test Film One (2020).nfo" <<'NFO'
<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<movie>
  <title>Test Film One</title>
  <originaltitle>Film Un d'Essai</originaltitle>
  <year>2020</year>
  <premiered>2020-04-12</premiered>
  <plot>A surveyor maps a canyon that keeps changing shape.</plot>
  <tagline>Every map is a guess.</tagline>
  <mpaa>PG-13</mpaa>
  <country>Ireland</country>
  <genre>Science Fiction</genre>
  <genre>Drama</genre>
  <studio>Lantern Pictures</studio>
  <director>Joon Park</director>
  <credits>Ines Okafor</credits>
  <rating>7.4</rating>
  <uniqueid type="tmdb" default="true">990001</uniqueid>
  <uniqueid type="imdb">tt9900001</uniqueid>
  <trailer>plugin://plugin.video.youtube/?action=play_video&amp;videoid=dQw4w9WgXcQ</trailer>
  <actor><name>Mara Ellison</name><role>Ada Quill</role><order>0</order></actor>
  <actor><name>Tomas Reyes</name><role>The Ranger</role><order>1</order></actor>
</movie>
NFO

  video "$film_two/Test Film Two (2021).mkv" hevc-ac3
  image "$film_two/poster.jpg" 600x900 maroon
  image "$film_two/fanart.jpg" 1920x1080 olive
  cat > "$film_two/Test Film Two (2021).nfo" <<'NFO'
<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<movie>
  <title>Test Film Two</title>
  <year>2021</year>
  <premiered>2021-09-03</premiered>
  <plot>Two lighthouse keepers trade letters across a strait.</plot>
  <mpaa>R</mpaa>
  <genre>Romance</genre>
  <rating>6.1</rating>
  <uniqueid type="tmdb" default="true">990002</uniqueid>
  <director>Saoirse Lane</director>
  <actor><name>Ilse Brandt</name><role>Wren</role><order>0</order></actor>
</movie>
NFO

  image "$show/poster.jpg" 600x900 purple
  image "$show/fanart.jpg" 1920x1080 darkgreen
  image "$show/season01-poster.jpg" 600x900 orange
  image "$show/season02-poster.jpg" 600x900 gold
  cat > "$show/tvshow.nfo" <<'NFO'
<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<tvshow>
  <title>Test Show</title>
  <year>2019</year>
  <premiered>2019-01-07</premiered>
  <plot>A small-town radio station stays on the air through one long winter.</plot>
  <mpaa>TV-14</mpaa>
  <genre>Drama</genre>
  <genre>Comedy</genre>
  <studio>Northwind Broadcasting</studio>
  <rating>8.1</rating>
  <uniqueid type="tmdb" default="true">990010</uniqueid>
  <uniqueid type="imdb">tt9900010</uniqueid>
  <actor><name>Nadia Brooks</name><role>Host</role><order>0</order></actor>
</tvshow>
NFO
  local season episode
  for season in 1 2; do
    local folder="$show/Season 0$season"
    cat > "$folder/season.nfo" <<NFO
<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<season>
  <title>Season $season</title>
  <seasonnumber>$season</seasonnumber>
  <plot>The station's season $season.</plot>
</season>
NFO
    for episode in 1 2; do
      local name="Test Show - S0${season}E0${episode}"
      video "$folder/$name.mkv" episode
      image "$folder/$name-thumb.jpg" 640x360 steelblue
      cat > "$folder/$name.nfo" <<NFO
<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<episodedetails>
  <title>Broadcast $season.$episode</title>
  <season>$season</season>
  <episode>$episode</episode>
  <aired>$((2018 + season))-0$episode-07</aired>
  <plot>Episode $episode of season $season.</plot>
</episodedetails>
NFO
    done
  done
}

# MARK: - Servers

# Waits until a server answers its public info.
wait_for() {
  local base="$1" name="$2"
  for _ in {1..120}; do
    if curl -fsS "$base/System/Info/Public" >/dev/null 2>&1; then return; fi
    sleep 1
  done
  echo "$name didn't start within two minutes." >&2
  exit 1
}

# An API call with the client header, and a token when given.
api() {
  local method="$1" url="$2" body="${3:-}" token="${4:-}"
  local auth="$CLIENT_HEADER"
  [[ -n "$token" ]] && auth="$auth, Token=\"$token\""
  local args=(-fsS -X "$method" -H "Authorization: $auth" -H "X-Emby-Authorization: $auth" -H "Content-Type: application/json")
  [[ -n "$token" ]] && args+=(-H "X-Emby-Token: $token")
  [[ -n "$body" ]] && args+=(--data "$body")
  curl "${args[@]}" "$url"
}

sign_in() {
  local base="$1"
  api POST "$base/Users/AuthenticateByName" "{\"Username\":\"$TEST_USER\",\"Pw\":\"$TEST_PASSWORD\"}" | jq -r .AccessToken
}

# Library options that keep the servers offline (no metadata or artwork
# lookups, the .nfo files and local images are used) and quick (no chapter
# images, trickplay or intro detection).
library_options() {
  local folder="$1" type="$2" types
  case "$type" in
    movies) types='["Movie"]' ;;
    tvshows) types='["Series","Season","Episode"]' ;;
  esac
  jq -n --arg folder "$folder" --argjson types "$types" '{
    LibraryOptions: {
      PathInfos: [{Path: $folder}],
      EnableRealtimeMonitor: false,
      EnableChapterImageExtraction: false,
      ExtractChapterImagesDuringLibraryScan: false,
      EnableTrickplayImageExtraction: false,
      ExtractTrickplayImagesDuringLibraryScan: false,
      EnableMarkerDetection: false,
      EnableMarkerDetectionDuringLibraryScan: false,
      EnableInternetProviders: false,
      SaveLocalMetadata: false,
      DownloadImagesInAdvance: false,
      ImportMissingEpisodes: false,
      MetadataSavers: [],
      TypeOptions: [$types[] | {Type: ., MetadataFetchers: [], ImageFetchers: [], MetadataFetcherOrder: [], ImageFetcherOrder: []}]
    }
  }'
}

add_libraries() {
  local base="$1" token="$2"
  # The folder goes in the address too: Jellyfin reads it only from there.
  api POST "$base/Library/VirtualFolders?name=Films&collectionType=movies&paths=%2Fmedia%2FMovies&refreshLibrary=false" \
    "$(library_options /media/Movies movies)" "$token" >/dev/null
  api POST "$base/Library/VirtualFolders?name=Shows&collectionType=tvshows&paths=%2Fmedia%2FShows&refreshLibrary=false" \
    "$(library_options /media/Shows tvshows)" "$token" >/dev/null
  api POST "$base/Library/Refresh" "" "$token" >/dev/null
}

# Waits for the scan to find both films, the series and its four episodes.
wait_for_scan() {
  local base="$1" token="$2" name="$3" counts
  for _ in {1..180}; do
    counts="$(api GET "$base/Items/Counts" "" "$token" 2>/dev/null || true)"
    if [[ -n "$counts" ]] && [[ "$(jq -r '"\(.MovieCount) \(.SeriesCount) \(.EpisodeCount)"' <<<"$counts")" == "2 1 4" ]]; then
      # Let the scan finish reading images and details after the items appear.
      sleep 5
      return
    fi
    sleep 1
  done
  echo "$name didn't finish scanning the test media within three minutes." >&2
  exit 1
}

start_jellyfin() {
  local base="http://localhost:$JELLYFIN_PORT"
  docker run -d --name "$JELLYFIN_CONTAINER" -p "${JELLYFIN_PORT}:8096" \
    -v "${MEDIA}:/media:ro" "$JELLYFIN_IMAGE" >/dev/null
  wait_for "$base" Jellyfin
  # The startup wizard, through its API.
  for _ in {1..30}; do api GET "$base/Startup/User" >/dev/null 2>&1 && break; sleep 1; done
  api POST "$base/Startup/Configuration" '{"UICulture":"en-US","MetadataCountryCode":"US","PreferredMetadataLanguage":"en"}' >/dev/null
  api POST "$base/Startup/User" "{\"Name\":\"$TEST_USER\",\"Password\":\"$TEST_PASSWORD\"}" >/dev/null
  api POST "$base/Startup/RemoteAccess" '{"EnableRemoteAccess":true,"EnableAutomaticPortMapping":false}' >/dev/null
  api POST "$base/Startup/Complete" >/dev/null
  local token
  token="$(sign_in "$base")"
  add_libraries "$base" "$token"
  wait_for_scan "$base" "$token" Jellyfin
}

start_emby() {
  local base="http://localhost:$EMBY_PORT"
  docker run -d --name "$EMBY_CONTAINER" -p "${EMBY_PORT}:8096" \
    -v "${MEDIA}:/media:ro" "$EMBY_IMAGE" >/dev/null
  wait_for "$base" Emby
  api POST "$base/Startup/Configuration" '{"UICulture":"en-US","MetadataCountryCode":"US","PreferredMetadataLanguage":"en"}' >/dev/null
  api POST "$base/Startup/User" "{\"Name\":\"$TEST_USER\",\"Password\":\"$TEST_PASSWORD\"}" >/dev/null
  api POST "$base/Startup/Complete" >/dev/null
  local token
  token="$(sign_in "$base")"
  add_libraries "$base" "$token"
  wait_for_scan "$base" "$token" Emby
}

for tool in docker ffmpeg curl jq; do
  command -v "$tool" >/dev/null || { echo "$tool is needed." >&2; exit 1; }
done

echo "Making test media in $MEDIA (once)…"
make_media
stop_servers
echo "Starting Jellyfin on port $JELLYFIN_PORT…"
start_jellyfin
echo "Starting Emby on port $EMBY_PORT…"
start_emby

cat <<DONE

Both servers are up with two films and a two-season series.

  Jellyfin  http://localhost:$JELLYFIN_PORT
  Emby      http://localhost:$EMBY_PORT
  Username  $TEST_USER
  Password  $TEST_PASSWORD   (throwaway; these servers hold only test media)

The integration test reads them from the environment:

  export XORVIO_TEST_JELLYFIN=http://localhost:$JELLYFIN_PORT
  export XORVIO_TEST_EMBY=http://localhost:$EMBY_PORT
  export XORVIO_TEST_SERVER_USER=$TEST_USER XORVIO_TEST_SERVER_PASSWORD=$TEST_PASSWORD
  cargo test -p xorvio-core --test media_servers_docker -- --ignored --nocapture

Stop them with: Scripts/test-servers/jellyfin-emby.sh stop
DONE
