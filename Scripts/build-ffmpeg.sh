#!/bin/zsh
# Builds the ffmpeg and ffprobe a release of Xorvio bundles, from upstream
# source releases, and puts them where Tauri takes sidecars from:
# src-tauri/binaries/xorvio-ffmpeg-<target> and xorvio-ffprobe-<target>, which
# src-tauri/tauri.ffmpeg.conf.json bundles (Scripts/build-app.sh --with-ffmpeg).
# The names keep a Linux package's copies in /usr/bin apart from the
# distribution's ffmpeg.
#
# Only what Xorvio asks of ffmpeg is built in (the lists below say where
# each part is used), and every library is linked in, so each binary stands
# alone and THIRD-PARTY.md names everything in it. x264 makes the result
# GPL: version 2 or later, or 3 or later on Linux, where OpenSSL's Apache
# licence joins it. Xorvio's own GPL-3.0-or-later covers both.
#
#   Scripts/build-ffmpeg.sh                                  for this computer
#   Scripts/build-ffmpeg.sh --target x86_64-apple-darwin     for Intel Macs, on Apple silicon too
#   Scripts/build-ffmpeg.sh --target universal-apple-darwin  for both, one file each
#   Scripts/build-ffmpeg.sh --force                          build again though nothing changed
#
# It needs a C compiler, make, curl, zlib and Python 3.7 or later: Xcode or
# the Command Line Tools on macOS; build-essential, zlib1g-dev and python3 on
# Linux; MSYS2's UCRT64 shell with its gcc and zlib on Windows. pkg-config,
# Meson, Ninja and (for x86-64) NASM come from PATH, or are built from their
# own pinned sources when they're missing. Everything is downloaded into
# build/ffmpeg/ and checked against the checksums here before it's used.
set -euo pipefail

ROOT="${0:A:h:h}"
WORK="$ROOT/build/ffmpeg"
DOWNLOADS="$WORK/downloads"
TOOLS="$WORK/tools"
SIDECARS="$ROOT/src-tauri/binaries"

# The releases, each pinned to its SHA-256. ffmpeg's is the tarball its
# release signing key (FCF986EA15E6E293A5644F10B4322F04D67658D8) signed.
FFMPEG_VERSION=9.0.2
FFMPEG_URL="https://ffmpeg.org/releases/ffmpeg-$FFMPEG_VERSION.tar.xz"
FFMPEG_SHA256=8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e
# x264 publishes no release tarballs. This is the head of its stable branch,
# as code.videolan.org's archive of that commit.
X264_COMMIT=b35605ace3ddf7c1a5d67a2eb553f034aef41d55
X264_URL="https://code.videolan.org/videolan/x264/-/archive/$X264_COMMIT/x264-$X264_COMMIT.tar.bz2"
X264_SHA256=6eeb82934e69fd51e043bd8c5b0d152839638d1ce7aa4eea65a3fedcf83ff224
# dav1d decodes AV1 for the remux: ffmpeg's own AV1 decoder needs a
# hardware decoder to do anything.
DAV1D_VERSION=1.5.4
DAV1D_URL="https://downloads.videolan.org/pub/videolan/dav1d/$DAV1D_VERSION/dav1d-$DAV1D_VERSION.tar.xz"
DAV1D_SHA256=686616b7c69eb88d44459391ab25cac13b6647a3b288835c5784e71c1514a5c5
# TLS on Linux, which has no system TLS ffmpeg can use. macOS and Windows
# use their own: Secure Transport and Schannel.
OPENSSL_VERSION=3.5.9
OPENSSL_URL="https://github.com/openssl/openssl/releases/download/openssl-$OPENSSL_VERSION/openssl-$OPENSSL_VERSION.tar.gz"
OPENSSL_SHA256=603f5602e2eef00d77fbd429d34dcd5822bb301757a1bc9cdb24c670f1eb859a

# Build tools, built only when they aren't installed. Meson 1.11 is the
# last that runs on the Python 3.9 macOS comes with.
PKGCONF_VERSION=3.0.10
PKGCONF_URL="https://distfiles.ariadne.space/pkgconf/pkgconf-$PKGCONF_VERSION.tar.xz"
PKGCONF_SHA256=9371b2f6372da574d4c6dc96fe9b1463037bcf6afcb6671c1555afde7b17e2de
MESON_VERSION=1.11.2
MESON_URL="https://github.com/mesonbuild/meson/releases/download/$MESON_VERSION/meson-$MESON_VERSION.tar.gz"
MESON_SHA256=698feae069cef3ecd4d7aaf281d7df359bdfcf555a9a1564383d3b913fa8a736
NINJA_VERSION=1.13.2
NINJA_URL="https://github.com/ninja-build/ninja/archive/refs/tags/v$NINJA_VERSION.tar.gz"
NINJA_SHA256=974d6b2f4eeefa25625d34da3cb36bdcebe7fbce40f4c16ac0835fd1c0cbae17
NASM_VERSION=2.16.03
NASM_URL="https://www.nasm.us/pub/nasm/releasebuilds/$NASM_VERSION/nasm-$NASM_VERSION.tar.xz"
NASM_SHA256=1412a1c760bbd05db026b6c0d1657affd6631cd0a63cddb6f73cc6d4aa616148

# What crates/xorvio-media asks of ffmpeg and ffprobe: the gateway (remux.rs,
# probe.rs), the live buffer (timeshift/plan.rs) and the recorder
# (recorder/job.rs, recorder/files.rs). They read providers' streams and
# playlists over http and https (HLS keys through crypto), files on this
# computer, and their own pipes; a proxy set in http_proxy is honoured.
PROTOCOLS=(file pipe http https tcp tls httpproxy crypto data)
# The containers sources and files come in, the HLS playlists the live
# buffer and the recorder read, the recorder's list of parts (concat), and
# seek-bar thumbnails' raw tiles read back to make sheets (rawvideo).
DEMUXERS=(
  mpegts hls flv live_flv mov matroska avi asf mpegps mpegvideo m4v h264 hevc
  ogg wav aac ac3 eac3 dts truehd mp3 flac rm wtv dv concat rawvideo
)
# ffprobe needs a stream's decoder to describe it fully, and the remux
# decodes what the web view can't play to encode it again: video other than
# H.264 and HEVC (VP9 and AV1 too, which MPEG-TS can't carry; AV1 through
# dav1d), audio other than AAC, MP3 and Dolby, text subtitles to write as
# WebVTT, and picture subtitles (PGS, VobSub, DVB) to draw into the video.
# Seek-bar thumbnails' raw tiles are read back to lay out sheets (rawvideo).
DECODERS=(
  rawvideo h264 hevc mpeg1video mpeg2video mpeg4 msmpeg4v1 msmpeg4v2 msmpeg4v3 h263 h263i h263p flv
  vp6 vp6f vp8 vp9 libdav1d vc1 wmv1 wmv2 wmv3 theora mjpeg dvvideo prores rv30 rv40
  aac aac_latm ac3 eac3 dca truehd mlp mp1 mp2 mp3 flac vorbis opus alac wmav1 wmav2 wmapro
  cook amrnb amrwb s302m dvaudio
  pcm_s16le pcm_s16be pcm_s24le pcm_s24be pcm_s32le pcm_f32le pcm_u8 pcm_alaw pcm_mulaw pcm_bluray pcm_dvd
  subrip srt ass ssa webvtt movtext text pgssub dvdsub dvbsub
)
# Finding frames and parameters in those codecs' streams.
PARSERS=(
  h264 hevc mpegvideo mpeg4video h263 vc1 vp3 vp8 vp9 av1 mjpeg rv34
  aac aac_latm ac3 dca mlp mpegaudio flac vorbis opus cook dvaudio dvbsub dvdsub
)
# Video the web view can't decode becomes H.264, other audio AAC, and text
# subtitles WebVTT; seek-bar thumbnails are raw tiles, then JPEG sheets; and
# finding ad breaks hands its frames to the null muxer, whose own encoders
# are wrapped_avframe and pcm_s16le.
ENCODERS=(libx264 aac webvtt rawvideo mjpeg wrapped_avframe pcm_s16le)
# The remux writes MPEG-TS and WebVTT, the live buffer HLS of fragmented
# MP4 (hls with mp4), the recorder MPEG-TS; thumbnails write raw tiles and
# JPEG sheets (rawvideo, image2), and finding ad breaks writes nothing (null).
MUXERS=(mpegts hls mp4 webvtt rawvideo image2 null)
# Deinterlacing (yadif), 1080p at most and pictures fitted to the video
# (scale), picture subtitles drawn in (overlay, null), standard channel
# layouts (aformat); scale and aresample also convert formats between
# filters. Thumbnails square their pixels (setsar) and lay tiles out in
# sheets (tile); ad breaks are found by black and silence (blackdetect,
# silencedetect). ffmpeg's own tool brings format, anull, trim and atrim
# with it.
FILTERS=(yadif scale overlay null aformat aresample setsar tile blackdetect silencedetect)
# Copied AAC into MP4 (aac_adtstoasc), the remux's audio delay (setts),
# H.264 and HEVC from MP4 or Matroska into MPEG-TS, and parameter sets
# found while probing (extract_extradata). The MP4 muxer adds vp9_superframe.
BSFS=(aac_adtstoasc setts h264_mp4toannexb hevc_mp4toannexb vp9_superframe extract_extradata)

die() {
  print -u2 -- "build-ffmpeg: $*"
  exit 1
}

TARGET=""
FORCE=0
while (( $# )); do
  case "$1" in
    --target) TARGET="${2:-}"; shift ;;
    --target=*) TARGET="${1#*=}" ;;
    --force) FORCE=1 ;;
    -h|--help) sed -n '2,23p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) die "unknown option $1" ;;
  esac
  shift
done

# The Rust target triple Tauri names sidecars by.
HOST_ARCH="${$(uname -m)/arm64/aarch64}"
case "$(uname -s)" in
  Darwin) HOST="$HOST_ARCH-apple-darwin" ;;
  Linux) HOST="$HOST_ARCH-unknown-linux-gnu" ;;
  MINGW*|UCRT*|CLANG*) HOST="$HOST_ARCH-pc-windows-msvc" ;;
  *) die "this script builds on macOS, Linux and Windows (in MSYS2's UCRT64 shell), not $(uname -s)" ;;
esac
TARGET="${TARGET:-$HOST}"

# A universal Mac build is the two Mac builds in one file each.
if [[ "$TARGET" == universal-apple-darwin ]]; then
  [[ "$HOST" == *-apple-darwin ]] || die "universal-apple-darwin builds on a Mac"
  force=(); (( FORCE )) && force=(--force)
  for arch in aarch64 x86_64; do zsh "$0" --target "$arch-apple-darwin" $force; done
  for tool in xorvio-ffmpeg xorvio-ffprobe; do
    lipo -create -output "$SIDECARS/$tool-$TARGET" "$SIDECARS/$tool-aarch64-apple-darwin" "$SIDECARS/$tool-x86_64-apple-darwin"
  done
  print "Made $SIDECARS/xorvio-{ffmpeg,ffprobe}-$TARGET"
  exit 0
fi

ARCH="${TARGET%%-*}"
EXE=""
ARCH_FLAGS=()
CROSS=0
case "$TARGET" in
  aarch64-apple-darwin|x86_64-apple-darwin)
    [[ "$HOST" == *-apple-darwin ]] || die "$TARGET builds on a Mac"
    [[ "$ARCH" != "$HOST_ARCH" ]] && CROSS=1
    # As far back as the app goes (tauri.conf.json's minimumSystemVersion).
    MACOS_MIN="$(sed -n 's/.*"minimumSystemVersion": *"\([^"]*\)".*/\1/p' "$ROOT/src-tauri/tauri.conf.json")"
    export MACOSX_DEPLOYMENT_TARGET="${MACOS_MIN:-12.0}"
    ARCH_FLAGS=(-arch "${ARCH/aarch64/arm64}")
    export CC=clang CXX=clang++
    TLS=(--enable-securetransport)
    THREADS=(--enable-pthreads)
    ;;
  x86_64-unknown-linux-gnu|aarch64-unknown-linux-gnu)
    [[ "$TARGET" == "$HOST" ]] || die "$TARGET builds only on $TARGET"
    export CC="${CC:-cc}" CXX="${CXX:-c++}"
    # OpenSSL is Apache-2.0, which goes with GPL version 3 but not 2.
    TLS=(--enable-openssl --enable-version3)
    THREADS=(--enable-pthreads)
    ;;
  x86_64-pc-windows-msvc)
    [[ "$TARGET" == "$HOST" ]] || die "$TARGET builds only in MSYS2's UCRT64 shell on Windows"
    # MSYS2's gcc; the binaries don't depend on its runtime (-static), so they
    # sit beside the app's MSVC-built executable as they are.
    export CC=gcc CXX=g++
    EXE=.exe
    TLS=(--enable-schannel)
    THREADS=(--enable-w32threads)
    ;;
  *) die "no build for $TARGET" ;;
esac

BUILD="$WORK/$TARGET"
DEPS="$BUILD/deps"
JOBS="$(sysctl -n hw.ncpu 2>/dev/null || nproc 2>/dev/null || print 4)"
# Only these builds' own .pc files, never what else the computer has.
export PKG_CONFIG_LIBDIR="$DEPS/lib/pkgconfig"
unset PKG_CONFIG_PATH
path=("$TOOLS/bin" $path)

# Built already, from this script as it is now?
STAMP="$BUILD/stamp"
FFMPEG_OUT="$SIDECARS/xorvio-ffmpeg-$TARGET$EXE"
FFPROBE_OUT="$SIDECARS/xorvio-ffprobe-$TARGET$EXE"
sha256() {
  if (( $+commands[sha256sum] )); then sha256sum -- "$1"; else shasum -a 256 -- "$1"; fi | awk '{ print $1 }'
}
SCRIPT_SUM="$(sha256 "$0")"
if (( ! FORCE )) && [[ -x "$FFMPEG_OUT" && -x "$FFPROBE_OUT" && "$(cat "$STAMP" 2>/dev/null)" == "$SCRIPT_SUM" ]]; then
  print "ffmpeg for $TARGET is up to date in src-tauri/binaries (--force builds it again)."
  exit 0
fi

for tool in curl tar make; do
  (( $+commands[$tool] )) || die "$tool is needed and isn't installed"
done
(( $+commands[$CC] )) || die "a C compiler ($CC) is needed and isn't installed"
[[ "$TARGET" == *-linux-gnu ]] && { (( $+commands[perl] )) || die "perl is needed to build OpenSSL"; }

# fetch <url> <sha256>: the file, downloaded once into build/ffmpeg/downloads
# and checked against its pinned checksum, in REPLY.
fetch() {
  local url=$1 sum=$2 file="$DOWNLOADS/${1:t}"
  mkdir -p "$DOWNLOADS"
  if [[ ! -f "$file" ]]; then
    print "Downloading $url"
    curl -fsSL --retry 3 -o "$file.part" "$url"
    mv "$file.part" "$file"
  fi
  local actual="$(sha256 "$file")"
  if [[ "$actual" != "$sum" ]]; then
    rm -f "$file"
    die "${file:t} isn't the file that was pinned (its SHA-256 is $actual), so it was deleted and nothing was built from it"
  fi
  REPLY="$file"
}

# unpack <url> <sha256> <folder> <into>: a fresh copy of the source in
# <into>/<folder>, also in REPLY.
unpack() {
  fetch "$1" "$2"
  mkdir -p "$4"
  rm -rf "$4/$3"
  tar -xf "$REPLY" -C "$4"
  [[ -d "$4/$3" ]] || die "${REPLY:t} didn't hold $3"
  REPLY="$4/$3"
}

# quietly <log> <command…>: runs the command with what it prints kept in
# build/ffmpeg/<target>/logs/<log>.log (upstream builds print screenfuls of
# warnings), and shows the end of that only when it fails.
LOGS="$WORK/$TARGET/logs"
rm -rf "$LOGS"
mkdir -p "$LOGS"
quietly() {
  local log="$LOGS/$1.log"
  shift
  if ! "$@" >>"$log" 2>&1; then
    tail -40 "$log" >&2
    die "$1 failed in $PWD (the end of ${log#$ROOT/} is above)"
  fi
}

# Build tools that aren't installed, built for this computer into
# build/ffmpeg/tools (on PATH above).
bootstrap() {
  local tool=$1
  if (( $+commands[$tool] )); then
    return 0
  fi
  print "Building $tool (it isn't installed)"
  mkdir -p "$TOOLS/bin"
  (
    # For this computer, whatever the target.
    unset CC CXX MACOSX_DEPLOYMENT_TARGET
    case "$tool" in
      pkg-config)
        unpack "$PKGCONF_URL" "$PKGCONF_SHA256" "pkgconf-$PKGCONF_VERSION" "$TOOLS/src"
        cd "$REPLY"
        quietly pkgconf ./configure --disable-shared --prefix="$TOOLS"
        quietly pkgconf make -j"$JOBS"
        cp "pkgconf$EXE" "$TOOLS/bin/pkg-config$EXE"
        ;;
      meson)
        (( $+commands[python3] )) || die "Meson needs Python 3.7 or later, and python3 isn't installed"
        python3 -c 'import sys; sys.exit(sys.version_info < (3, 7))' || die "Meson needs Python 3.7 or later"
        unpack "$MESON_URL" "$MESON_SHA256" "meson-$MESON_VERSION" "$TOOLS/src"
        print -r -- "#!/bin/sh
exec python3 '$REPLY/meson.py' \"\$@\"" >"$TOOLS/bin/meson"
        chmod +x "$TOOLS/bin/meson"
        ;;
      ninja)
        (( $+commands[python3] )) || die "building Ninja needs Python 3, and python3 isn't installed"
        unpack "$NINJA_URL" "$NINJA_SHA256" "ninja-$NINJA_VERSION" "$TOOLS/src"
        cd "$REPLY"
        quietly ninja python3 configure.py --bootstrap
        cp "ninja$EXE" "$TOOLS/bin/"
        ;;
      nasm)
        unpack "$NASM_URL" "$NASM_SHA256" "nasm-$NASM_VERSION" "$TOOLS/src"
        cd "$REPLY"
        quietly nasm ./configure
        quietly nasm make -j"$JOBS" "nasm$EXE"
        cp "nasm$EXE" "$TOOLS/bin/"
        ;;
    esac
  )
  rehash
}

bootstrap pkg-config
bootstrap meson
bootstrap ninja
[[ "$ARCH" == x86_64 ]] && bootstrap nasm

rm -rf "$DEPS"
mkdir -p "$DEPS"

# OpenSSL, on Linux. /etc/ssl is where Debian, Ubuntu and Arch keep the
# system's certificates, which ffmpeg checks servers against; on other
# distributions Xorvio points SSL_CERT_FILE at theirs (process.rs).
if [[ "$TARGET" == *-linux-gnu ]]; then
  unpack "$OPENSSL_URL" "$OPENSSL_SHA256" "openssl-$OPENSSL_VERSION" "$BUILD"
  print "Building OpenSSL $OPENSSL_VERSION"
  (
    cd "$REPLY"
    quietly openssl ./Configure "linux-$ARCH" --prefix="$DEPS" --libdir=lib --openssldir=/etc/ssl \
      no-shared no-tests no-docs no-apps
    quietly openssl make -j"$JOBS" build_libs
    quietly openssl make install_dev
  )
fi

# x264, as a library, for the only kind of video Xorvio encodes:
# 8-bit 4:2:0 (-pix_fmt yuv420p).
X264_CONFIGURE=(
  --prefix="$DEPS" --enable-static --enable-pic --disable-cli
  --bit-depth=8 --chroma-format=420
  --disable-opencl --disable-avs --disable-swscale --disable-lavf --disable-ffms --disable-gpac --disable-lsmash
)
(( CROSS )) && X264_CONFIGURE+=(--host="$TARGET")
# Named, since MSYS2's UCRT64 shell calls itself something x264's config.guess doesn't know.
[[ "$TARGET" == *-windows-msvc ]] && X264_CONFIGURE+=(--host=x86_64-w64-mingw32)
(( $#ARCH_FLAGS )) && X264_CONFIGURE+=(--extra-cflags="$ARCH_FLAGS" --extra-ldflags="$ARCH_FLAGS")
# Arm assembly goes through the compiler; x86's through NASM, which takes no -arch.
[[ $#ARCH_FLAGS -gt 0 && "$ARCH" == aarch64 ]] && X264_CONFIGURE+=(--extra-asflags="$ARCH_FLAGS")
unpack "$X264_URL" "$X264_SHA256" "x264-$X264_COMMIT" "$BUILD"
print "Building x264 ${X264_COMMIT[1,8]}"
(
  cd "$REPLY"
  quietly x264 ./configure "${X264_CONFIGURE[@]}"
  quietly x264 make -j"$JOBS"
  quietly x264 make install-lib-static
)

# dav1d, as a library, 8- and 10-bit (HDR video is 10-bit).
DAV1D_CONFIGURE=(
  --prefix="$DEPS" --libdir=lib --buildtype=release --default-library=static
  -Denable_tools=false -Denable_tests=false -Denable_examples=false -Denable_docs=false
)
unpack "$DAV1D_URL" "$DAV1D_SHA256" "dav1d-$DAV1D_VERSION" "$BUILD"
if (( CROSS )); then
  print -r -- "[binaries]
c = ['clang', '-arch', '${ARCH_FLAGS[2]}']
ar = 'ar'
strip = 'strip'
nasm = 'nasm'

[host_machine]
system = 'darwin'
cpu_family = '$ARCH'
cpu = '$ARCH'
endian = 'little'" >"$BUILD/meson-cross.txt"
  DAV1D_CONFIGURE+=(--cross-file "$BUILD/meson-cross.txt")
fi
print "Building dav1d $DAV1D_VERSION"
(
  cd "$REPLY"
  quietly dav1d meson setup build "${DAV1D_CONFIGURE[@]}"
  quietly dav1d ninja -C build install
)

# ffmpeg itself. Nothing is found by looking around the computer
# (--disable-autodetect): everything in it is named here. Paths are relative
# so the configure line ffmpeg keeps (ffmpeg -buildconf) names no folder of
# the computer that built it.
FFMPEG_CONFIGURE=(
  --disable-everything --disable-autodetect
  --disable-shared --enable-static --pkg-config-flags=--static
  --disable-doc --disable-debug --disable-ffplay --disable-avdevice
  --enable-gpl --enable-libx264 --enable-libdav1d --enable-zlib
  $THREADS $TLS
  --enable-protocol="${(j:,:)PROTOCOLS}"
  --enable-demuxer="${(j:,:)DEMUXERS}"
  --enable-decoder="${(j:,:)DECODERS}"
  --enable-parser="${(j:,:)PARSERS}"
  --enable-encoder="${(j:,:)ENCODERS}"
  --enable-muxer="${(j:,:)MUXERS}"
  --enable-filter="${(j:,:)FILTERS}"
  --enable-bsf="${(j:,:)BSFS}"
)
(( CROSS )) && FFMPEG_CONFIGURE+=(--enable-cross-compile --arch="$ARCH" --target-os=darwin)
(( $#ARCH_FLAGS )) && FFMPEG_CONFIGURE+=(--extra-cflags="$ARCH_FLAGS" --extra-ldflags="$ARCH_FLAGS")
# Windows by name (as for x264), and the MinGW runtime and threads linked
# in, so nothing else has to be installed.
[[ "$TARGET" == *-windows-msvc ]] && FFMPEG_CONFIGURE+=(--target-os=mingw32 --extra-ldflags=-static)
unpack "$FFMPEG_URL" "$FFMPEG_SHA256" "ffmpeg-$FFMPEG_VERSION" "$BUILD"
FFMPEG_SRC="$REPLY"
print "Building ffmpeg $FFMPEG_VERSION"
(
  cd "$FFMPEG_SRC"
  if ! ./configure "${FFMPEG_CONFIGURE[@]}" >"$LOGS/ffmpeg.log" 2>&1; then
    tail -40 ffbuild/config.log >&2
    die "ffmpeg's configure failed (the end of its ffbuild/config.log is above)"
  fi
  quietly ffmpeg make -j"$JOBS"
)

mkdir -p "$SIDECARS"
cp "$FFMPEG_SRC/ffmpeg$EXE" "$FFMPEG_OUT"
cp "$FFMPEG_SRC/ffprobe$EXE" "$FFPROBE_OUT"

# A build that can run here should have what Xorvio uses.
if "$FFMPEG_OUT" -hide_banner -version >/dev/null 2>&1; then
  check() {
    local listed="$("$FFMPEG_OUT" -hide_banner "$1" 2>/dev/null)"
    listed="${${listed//$'\r'/}//$'\n'/ }"
    [[ " $listed " == *" $2 "* ]] || die "the build has no $2 (ffmpeg $1)"
  }
  check -encoders libx264
  check -decoders libdav1d
  check -filters overlay
  check -protocols https
elif [[ "$TARGET" == "$HOST" ]]; then
  die "the ffmpeg just built doesn't run"
fi

# What the release notes and the GPL's offer of source need: what went in,
# from where, and how it was configured. They're published, so a command
# line names this computer's folders by placeholders instead (the cross
# file's, say): <deps>, then <build>, then <repo>.
command_line() {
  local args=("${@//$DEPS/<deps>}")
  args=("${args[@]//$WORK/<build>}")
  args=("${args[@]//$ROOT/<repo>}")
  print -r -- "${(@q-)args}"
}
{
  print "ffmpeg and ffprobe for $TARGET, built by Scripts/build-ffmpeg.sh on $(date -u '+%Y-%m-%d')"
  print "Compiler: $("$CC" --version 2>/dev/null | head -1)"
  print
  print "ffmpeg $FFMPEG_VERSION  $FFMPEG_URL  sha256 $FFMPEG_SHA256"
  print "x264 $X264_COMMIT  $X264_URL  sha256 $X264_SHA256"
  print "dav1d $DAV1D_VERSION  $DAV1D_URL  sha256 $DAV1D_SHA256"
  [[ "$TARGET" == *-linux-gnu ]] && print "OpenSSL $OPENSSL_VERSION  $OPENSSL_URL  sha256 $OPENSSL_SHA256"
  print
  print "Folders: <repo> is Xorvio's source, <build> its build/ffmpeg, and <deps> <build>/$TARGET/deps, where x264 and dav1d were installed."
  print -r -- "x264: ./configure $(command_line "${X264_CONFIGURE[@]}")"
  print -r -- "dav1d: meson setup build $(command_line "${DAV1D_CONFIGURE[@]}")"
  print -r -- "ffmpeg: ./configure $(command_line "${FFMPEG_CONFIGURE[@]}")"
} >"$BUILD/BUILDINFO.txt"
print -r -- "$SCRIPT_SUM" >"$STAMP"

size() { print "$(( $(wc -c <"$1") / 1048576.0 ))" | awk '{ printf "%.1f MB", $1 }' }
print "Built $FFMPEG_OUT ($(size "$FFMPEG_OUT")) and ${FFPROBE_OUT:t} ($(size "$FFPROBE_OUT"))."
print "What went in and how: ${BUILD#$ROOT/}/BUILDINFO.txt"
