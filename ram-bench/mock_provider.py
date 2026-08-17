#!/usr/bin/env python3
"""Minimal OpenAI-compatible mock provider for RAM benchmarking.

Serves GET /v1/models and POST /v1/chat/completions (streaming SSE).
The stream is deliberately slow-ish (configurable chunk count/delay) so the
benchmark has a measurable "active run" window.
"""
import argparse
import json
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

MODEL_ID = "mock-gpt"
CHUNK_COUNT = 20
CHUNK_DELAY_MS = 50


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *args):
        pass

    def _send_json(self, obj, status=200):
        body = json.dumps(obj).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path == "/v1/models":
            self._send_json({
                "object": "list",
                "data": [{
                    "id": MODEL_ID,
                    "object": "model",
                    "owned_by": "bench",
                }],
            })
        else:
            self._send_json({"error": {"message": f"not found: {self.path}"}}, 404)

    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0))
        if length:
            self.rfile.read(length)
        if self.path == "/v1/chat/completions":
            self._stream_completion()
        else:
            self._send_json({"error": {"message": f"not found: {self.path}"}}, 404)

    def _stream_completion(self):
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "keep-alive")
        self.end_headers()

        def chunk(text, finish_reason=None):
            obj = {
                "id": "chatcmpl-bench",
                "object": "chat.completion.chunk",
                "created": int(time.time()),
                "model": MODEL_ID,
                "choices": [{
                    "index": 0,
                    "delta": {"content": text},
                    "finish_reason": finish_reason,
                }],
            }
            return f"data: {json.dumps(obj)}\n\n".encode()

        try:
            self.wfile.write(chunk("", None))
            self.wfile.flush()
            for i in range(CHUNK_COUNT):
                self.wfile.write(chunk(f"benchmark chunk {i} " * 4))
                self.wfile.flush()
                time.sleep(CHUNK_DELAY_MS / 1000.0)
            self.wfile.write(chunk("", "stop"))
            self.wfile.write(b"data: [DONE]\n\n")
            self.wfile.flush()
        except BrokenPipeError:
            pass


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=18766)
    ap.add_argument("--chunks", type=int, default=CHUNK_COUNT)
    ap.add_argument("--delay-ms", type=int, default=CHUNK_DELAY_MS)
    args = ap.parse_args()
    global CHUNK_COUNT, CHUNK_DELAY_MS
    CHUNK_COUNT = args.chunks
    CHUNK_DELAY_MS = args.delay_ms
    server = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    print(f"MOCK_READY port={args.port}", flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()