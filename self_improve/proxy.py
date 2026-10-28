#!/usr/bin/env python3
"""Minimal CONNECT proxy that only permits the LLM endpoint."""
import socket
import socketserver
import sys

ALLOW = {("llm.labile.cc", 443)}
PORT = 3128


class Proxy(socketserver.StreamRequestHandler):
    def handle(self):
        line = self.rfile.readline(8192).decode("latin-1").strip()
        if not line:
            return
        parts = line.split()
        if len(parts) != 3 or parts[0] != "CONNECT":
            self.wfile.write(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n")
            return
        target = parts[1]
        if ":" not in target:
            self.wfile.write(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n")
            return
        host, port = target.rsplit(":", 1)
        try:
            port = int(port)
        except ValueError:
            return
        if (host, port) not in ALLOW:
            print(f"proxy: denied {host}:{port}", file=sys.stderr, flush=True)
            self.wfile.write(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n")
            return
        try:
            upstream = socket.create_connection((host, port), timeout=15)
        except OSError:
            self.wfile.write(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n")
            return
        self.wfile.write(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        self.wfile.flush()
        self.connection.settimeout(None)
        upstream.settimeout(None)
        socketserver.ThreadingTCPServer  # keep reference
        import threading
        def pipe(src, dst):
            try:
                while True:
                    data = src.recv(65536)
                    if not data:
                        break
                    dst.sendall(data)
            except OSError:
                pass
            finally:
                try:
                    dst.shutdown(socket.SHUT_WR)
                except OSError:
                    pass
        t = threading.Thread(target=pipe, args=(self.connection, upstream), daemon=True)
        t.start()
        pipe(upstream, self.connection)
        upstream.close()


class Server(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True


if __name__ == "__main__":
    with Server(("0.0.0.0", PORT), Proxy) as server:
        server.serve_forever()
