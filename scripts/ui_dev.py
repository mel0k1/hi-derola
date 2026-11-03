import functools
import http.server
import os
import sys

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 8971
ROOT = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "ui", "dist")

if not os.path.isdir(ROOT):
    print("ui/dist is missing: run `npm run build` inside ui/ first")
    sys.exit(1)

class Handler(http.server.SimpleHTTPRequestHandler):
    def __init__(self, *a, **kw):
        super().__init__(*a, directory=ROOT, **kw)

    def end_headers(self):
        self.send_header("Cache-Control", "no-store")
        super().end_headers()

    def log_message(self, *a):
        pass

http.server.ThreadingHTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
