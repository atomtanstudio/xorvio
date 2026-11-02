# Third-party software in Xorvio

Xorvio is free software under the GNU General Public License, version 3 or later (see LICENSE). This page lists the software from others that its downloads carry, under what licence, and where its source is.

## ffmpeg and ffprobe

Every download includes `xorvio-ffmpeg` and `xorvio-ffprobe`, Xorvio's own copies of ffmpeg and ffprobe, next to the app's executable (`Xorvio.app/Contents/MacOS` on macOS, the install folder on Windows, `/usr/bin` or the AppImage's `usr/bin` on Linux). Xorvio runs them for playing what the web view can't, the live TV buffer, recording and reading what a stream holds. `Scripts/build-ffmpeg.sh` builds them from the upstream source releases below, with only the parts Xorvio uses, and every library linked in.

Built with x264, each copy as a whole is under the **GNU GPL, version 2 or later** on macOS and Windows, and **version 3 or later** on Linux, where OpenSSL's Apache licence joins it (`ffmpeg -L` says which). Both are compatible with Xorvio's GPL-3.0-or-later.

| Software | Version | Licence | Source |
| --- | --- | --- | --- |
| FFmpeg (libavcodec, libavformat, libavfilter, libavutil, libswscale, libswresample, and the ffmpeg and ffprobe tools) | 9.0.2 | LGPL-2.1-or-later, with GPL-2.0-or-later parts; GPL as built (`--enable-gpl`) | https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz (SHA-256 `8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e`) |
| x264, the H.264 encoder | stable branch, commit `b35605ace3ddf7c1a5d67a2eb553f034aef41d55` | GPL-2.0-or-later | https://code.videolan.org/videolan/x264/-/archive/b35605ace3ddf7c1a5d67a2eb553f034aef41d55/x264-b35605ace3ddf7c1a5d67a2eb553f034aef41d55.tar.bz2 (SHA-256 `6eeb82934e69fd51e043bd8c5b0d152839638d1ce7aa4eea65a3fedcf83ff224`) |
| dav1d, the AV1 decoder | 1.5.4 | BSD-2-Clause | https://downloads.videolan.org/pub/videolan/dav1d/1.5.4/dav1d-1.5.4.tar.xz (SHA-256 `686616b7c69eb88d44459391ab25cac13b6647a3b288835c5784e71c1514a5c5`) |
| OpenSSL (Linux only) | 3.5.9 | Apache-2.0 | https://github.com/openssl/openssl/releases/download/openssl-3.5.9/openssl-3.5.9.tar.gz (SHA-256 `603f5602e2eef00d77fbd429d34dcd5822bb301757a1bc9cdb24c670f1eb859a`) |
| zlib (Windows only; macOS and Linux use the system's) | MSYS2's UCRT64 package when built | Zlib | https://zlib.net |
| MinGW-w64 runtime, winpthreads and libgcc (Windows only) | MSYS2's UCRT64 GCC when built | ZPL-2.1 and public domain; MIT; GPL-3.0-or-later with the GCC Runtime Library Exception | https://www.mingw-w64.org, https://gcc.gnu.org |

HTTPS goes through the system's own TLS on macOS (Secure Transport) and Windows (Schannel), which are part of the operating system.

**Getting the source.** Every GitHub release carries, beside its downloads, the exact source tarballs above and each build's notes (`ffmpeg-<target>-BUILDINFO.txt`: versions, checksums, compiler and the configure lines). `Scripts/build-ffmpeg.sh` in the release's own source rebuilds them. `xorvio-ffmpeg -buildconf` prints the configuration of any copy.

## In the app itself

The interface bundles the Inter typeface (`@fontsource-variable/inter`, SIL Open Font License 1.1), hls.js and mpegts.js (Apache-2.0), React, React DOM, Zustand and TanStack Virtual (MIT), Lucide icons (ISC) and Tauri's API (Apache-2.0 or MIT). The Rust side is built from the crates in `Cargo.lock`, Tauri among them (Apache-2.0 or MIT), each under its own licence, which its source carries.
