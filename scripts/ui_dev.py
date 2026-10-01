import functools
import http.server
import sys

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 8971

class Handler(http.server.SimpleHTTPRequestHandler):
    def end_headers(self):
        self.send_header("Cache-Control", "no-store")
        super().end_headers()

    def log_message(self, *a):
        pass

http.server.ThreadingHTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
