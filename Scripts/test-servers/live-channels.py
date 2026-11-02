"""Live channels for the test servers (jellyfin-emby.sh): every .ts clip in
the folder is served as an endless live stream, looped and sent at its own
bit rate as a broadcast would be, so a server's tuner sees a stream that
never ends. Anything else in the folder (the playlist, the guide, logos) is
served as it is.

    python3 live-channels.py <folder> <port> <clip seconds>
"""

import http.server
import os
import sys
import time

ROOT, PORT, CLIP_SECONDS = sys.argv[1], int(sys.argv[2]), float(sys.argv[3])
# How far ahead of real time a reader may get, as a tuner's buffer would.
LEAD_SECONDS = 1.0


class Handler(http.server.SimpleHTTPRequestHandler):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, directory=ROOT, **kwargs)

    def do_GET(self):
        name = os.path.basename(self.path.split("?")[0])
        clip = os.path.join(ROOT, name)
        if not name.endswith(".ts"):
            return super().do_GET()
        if not os.path.isfile(clip):
            return self.send_error(404)
        self.send_response(200)
        self.send_header("Content-Type", "video/mp2t")
        self.end_headers()
        rate = os.path.getsize(clip) / CLIP_SECONDS
        started, sent = time.monotonic(), 0
        try:
            while True:
                with open(clip, "rb") as source:
                    while chunk := source.read(64 * 1024):
                        self.wfile.write(chunk)
                        sent += len(chunk)
                        ahead = sent / rate - (time.monotonic() - started)
                        if ahead > LEAD_SECONDS:
                            time.sleep(ahead - LEAD_SECONDS)
        except (BrokenPipeError, ConnectionResetError):
            pass


http.server.ThreadingHTTPServer(("", PORT), Handler).serve_forever()
