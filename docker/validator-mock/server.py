#!/usr/bin/env python3
"""Simple HTTP server that simulates a validator's metrics endpoint."""

import json
import os
import random
import time
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, HTTPServer


PORT = int(os.environ.get("VALIDATOR_METRICS_PORT", "9100"))
VALIDATOR_ID = os.environ.get("VALIDATOR_ID", "validator-local")
RANDOM = random.Random()
STATE = {
    "rpc_enabled": True,
    "rpc_throttled": False,
    "last_restart": 0,
    "scripts": [],
    "alerts": [],
}


def _metrics_payload() -> str:
    ts = int(time.time())
    slot_lag = RANDOM.randint(0, 150)
    vote_success = RANDOM.uniform(0.8, 1.0)
    cpu_usage = RANDOM.uniform(0.1, 0.95)
    ram_usage = RANDOM.uniform(8.0, 96.0)
    disk_usage = RANDOM.uniform(20.0, 95.0)
    rpc_qps = RANDOM.uniform(300.0, 1500.0)
    rpc_error_rate = RANDOM.uniform(0.0, 0.05)
    if not STATE["rpc_enabled"]:
        rpc_qps = RANDOM.uniform(0.0, 5.0)
        rpc_error_rate = 0.0
    elif STATE["rpc_throttled"]:
        rpc_qps = RANDOM.uniform(25.0, 250.0)
        rpc_error_rate = RANDOM.uniform(0.05, 0.2)
    if ts - STATE["last_restart"] < 30:
        slot_lag = min(slot_lag, 10)
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
        "# HELP validator_ram_usage_gb RAM usage in gigabytes",
        "# TYPE validator_ram_usage_gb gauge",
        f'validator_ram_usage_gb{{id="{VALIDATOR_ID}"}} {ram_usage:.2f}',
        "# HELP validator_disk_usage_pct Disk usage percentage",
        "# TYPE validator_disk_usage_pct gauge",
        f'validator_disk_usage_pct{{id="{VALIDATOR_ID}"}} {disk_usage:.2f}',
        "# HELP validator_rpc_qps RPC queries per second",
        "# TYPE validator_rpc_qps gauge",
        f'validator_rpc_qps{{id="{VALIDATOR_ID}"}} {rpc_qps:.2f}',
        "# HELP validator_rpc_error_rate RPC error rate fraction",
        "# TYPE validator_rpc_error_rate gauge",
        f'validator_rpc_error_rate{{id="{VALIDATOR_ID}"}} {rpc_error_rate:.4f}',
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

    def do_POST(self):  # noqa: N802 (stdlib hook)
        routes = {
            "/admin/rpc/disable": self._disable_rpc,
            "/admin/rpc/enable": self._enable_rpc,
            "/admin/rpc/throttle": self._throttle_rpc,
            "/admin/validator/restart": self._restart_validator,
            "/admin/maintenance/run": self._run_script,
            "/admin/alert": self._record_alert,
        }
        handler = routes.get(self.path)
        if handler is None:
            self._json_response({"error": "not_found"}, HTTPStatus.NOT_FOUND)
            return
        handler()

    def _disable_rpc(self) -> None:
        STATE["rpc_enabled"] = False
        STATE["rpc_throttled"] = False
        self._json_response({"status": "rpc_disabled"})

    def _enable_rpc(self) -> None:
        STATE["rpc_enabled"] = True
        STATE["rpc_throttled"] = False
        self._json_response({"status": "rpc_enabled"})

    def _throttle_rpc(self) -> None:
        STATE["rpc_enabled"] = True
        STATE["rpc_throttled"] = True
        self._json_response({"status": "rpc_throttled"})

    def _restart_validator(self) -> None:
        STATE["last_restart"] = int(time.time())
        self._json_response({"status": "restarted"})

    def _run_script(self) -> None:
        payload = self._read_json()
        script = payload.get("script", "unknown-script")
        STATE["scripts"].append({"name": script, "ts": int(time.time())})
        self._json_response({"status": "script_run", "script": script})

    def _record_alert(self) -> None:
        payload = self._read_json()
        message = payload.get("message", "")
        STATE["alerts"].append({"message": message, "ts": int(time.time())})
        self._json_response({"status": "alert_recorded"})

    def _json_response(self, payload, status: HTTPStatus = HTTPStatus.OK) -> None:
        body = json.dumps(payload).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _read_json(self):
        length = int(self.headers.get("Content-Length") or 0)
        if length == 0:
            return {}
        data = self.rfile.read(length)
        if not data:
            return {}
        try:
            return json.loads(data.decode("utf-8"))
        except json.JSONDecodeError:
            return {}

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

