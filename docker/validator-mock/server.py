#!/usr/bin/env python3
"""Simple HTTP server that simulates a validator's metrics endpoint."""

import os
import random
import time
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, HTTPServer


PORT = int(os.environ.get("VALIDATOR_METRICS_PORT", "9100"))
VALIDATOR_ID = os.environ.get("VALIDATOR_ID", "validator-local")
RANDOM = random.Random()


def _metrics_payload() -> str:
    ts = int(time.time())
    slot_lag = RANDOM.randint(0, 150)
    vote_success = RANDOM.uniform(0.8, 1.0)
    cpu_usage = RANDOM.uniform(0.1, 0.95)
    lines = [
        "# HELP validator_slot_lag Current slot lag",
        "# TYPE validator_slot_lag gauge",
        f'validator_slot_lag{{id="{VALIDATOR_ID}"}} {slot_lag}',
        "# HELP validator_vote_success_rate Vote success rate",
        "# TYPE validator_vote_success_rate gauge",
        f'validator_vote_success_rate{{id="{VALIDATOR_ID}"}} {vote_success:.4f}',
        "# HELP validator_cpu_usage CPU usage fraction",
        "# TYPE validator_cpu_usage gauge",
        f'validator_cpu_usage{{id="{VALIDATOR_ID}"}} {cpu_usage:.4f}',
        "# HELP validator_metrics_timestamp Timestamp of metrics generation",
        "# TYPE validator_metrics_timestamp gauge",
        f'validator_metrics_timestamp{{id="{VALIDATOR_ID}"}} {ts}',
        "",
    ]
    return "\n".join(lines)


class MetricsHandler(BaseHTTPRequestHandler):
    def do_GET(self):  # noqa: N802 (stdlib hook)
        if self.path != "/metrics":
            self.send_response(HTTPStatus.NOT_FOUND)
            self.end_headers()
            self.wfile.write(b"not found")
            return

        payload = _metrics_payload().encode("utf-8")
        self.send_response(HTTPStatus.OK)
        self.send_header("Content-Type", "text/plain; version=0.0.4")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def log_message(self, fmt, *args):  # noqa: D401
        """Silence default stdout logging to keep docker logs tidy."""
        return


def main() -> None:
    server = HTTPServer(("0.0.0.0", PORT), MetricsHandler)
    print(
        f"[validator-mock] serving metrics for {VALIDATOR_ID} on port {PORT}",
        flush=True,
    )
    server.serve_forever()


if __name__ == "__main__":
    main()

