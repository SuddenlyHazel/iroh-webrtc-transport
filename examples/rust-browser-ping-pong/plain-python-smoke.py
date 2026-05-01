#!/usr/bin/env python3
"""Serve a plain, non-Trunk HTML smoke page from the current build.

Run `trunk build` first. This script writes `dist/plain-python-smoke.html`
that imports the generated wasm-bindgen module directly, installs the explicit
single-Wasm worker factory, and then calls the exported app entry point.
"""

from __future__ import annotations

import argparse
import html
import http.server
from pathlib import Path
import socketserver


ROOT = Path(__file__).resolve().parent
DIST = ROOT / "dist"
TEMPLATE = ROOT / "python_index.html"
SMOKE_HTML = DIST / "plain-python-smoke.html"


def generated_app_assets() -> tuple[str, str]:
    js_files = sorted(
        path.name
        for path in DIST.glob("iroh-webrtc-rust-browser-ping-pong-*.js")
        if not path.name.endswith("_bg.js")
    )
    wasm_files = sorted(DIST.glob("iroh-webrtc-rust-browser-ping-pong-*_bg.wasm"))
    if len(js_files) != 1 or len(wasm_files) != 1:
        raise SystemExit(
            "expected one generated app .js and one generated app _bg.wasm in dist; "
            "run `trunk build` first"
        )
    return js_files[0], wasm_files[0].name


def write_smoke_html() -> None:
    app_js, app_wasm = generated_app_assets()
    html_text = (
        TEMPLATE.read_text(encoding="utf-8")
        .replace("__APP_JS__", html.escape(app_js, quote=True))
        .replace("__APP_WASM__", html.escape(app_wasm, quote=True))
    )
    SMOKE_HTML.write_text(html_text, encoding="utf-8")


def serve(port: int) -> None:
    handler = http.server.SimpleHTTPRequestHandler
    with socketserver.TCPServer(("127.0.0.1", port), handler) as server:
        print(f"Serving http://127.0.0.1:{port}/plain-python-smoke.html")
        server.serve_forever()


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, default=8766)
    args = parser.parse_args()

    if not DIST.exists():
        raise SystemExit("missing dist directory; run `trunk build` first")
    write_smoke_html()
    import os

    os.chdir(DIST)
    serve(args.port)


if __name__ == "__main__":
    main()
