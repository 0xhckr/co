#!/usr/bin/env python3
"""Serve task-owned release bytes on loopback for native package smoke tests."""
import functools
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import sys

if len(sys.argv) != 3:
    sys.exit("usage: windows-package-server.py ASSET_DIRECTORY PORT_FILE")
server = ThreadingHTTPServer(("127.0.0.1", 0), functools.partial(
    SimpleHTTPRequestHandler, directory=str(Path(sys.argv[1]).resolve())))
Path(sys.argv[2]).write_text(str(server.server_port), encoding="utf-8")
server.serve_forever()
