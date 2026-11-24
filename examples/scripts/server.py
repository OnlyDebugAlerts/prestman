"""Offline HTTP echo server for the bundled Prestman script examples."""
import json
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import parse_qs, urlsplit


class EchoHandler(BaseHTTPRequestHandler):
    def do_GET(self):
        self.echo()

    def do_POST(self):
        self.echo()

    def echo(self):
        url = urlsplit(self.path)
        if url.path != "/check":
            self.send_error(404)
            return
        raw_body = self.rfile.read(int(self.headers.get("Content-Length", "0")))
        try:
            body = json.loads(raw_body) if raw_body else None
        except (ValueError, UnicodeDecodeError):
            self.send_error(400, "Send a valid JSON body")
            return
        result = {
            "method": self.command,
            "query": {name: values[-1] for name, values in parse_qs(url.query).items()},
            "headers": {name.lower(): value for name, value in self.headers.items()},
            "body": body,
        }
        encoded = json.dumps(result, ensure_ascii=False).encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(encoded)))
        self.end_headers()
        self.wfile.write(encoded)


if __name__ == "__main__":
    print("Prestman script check: http://127.0.0.1:8765/check", flush=True)
    try:
        HTTPServer(("127.0.0.1", 8765), EchoHandler).serve_forever()
    except KeyboardInterrupt:
        pass
