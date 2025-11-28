#!/usr/bin/env bash

set -euxo pipefail

# CONFIG
SOLANA_VERSION="${SOLANA_VERSION:-v1.18.26}"
GITHUB_RELEASE_URL="https://github.com/solana-labs/solana/releases/download/${SOLANA_VERSION}/solana-release-x86_64-unknown-linux-gnu.tar.bz2"
CLUSTER="mainnet-beta"
LEDGER_DIR="/var/solana/ledger"
ACCOUNTS_DIR="/var/solana/accounts"
IDENTITY="/var/solana/validator-keypair.json"
LOG_FILE="/var/solana/validator.log"
ENTRYPOINTS=(
  "entrypoint.mainnet-beta.solana.com:8001"
  "entrypoint2.mainnet-beta.solana.com:8001"
  "entrypoint3.mainnet-beta.solana.com:8001"
  "entrypoint4.mainnet-beta.solana.com:8001"
  "entrypoint5.mainnet-beta.solana.com:8001"
)
GENESIS_HASH="5eykt4UsFv8P8NJdTREpY1vzqKqZKvdpKuc147dw2N9d"
RPC_PORT=8899
RPC_BIND="0.0.0.0"
DYNAMIC_PORT_RANGE="8002-8020"   # leave 8000/8001 for gossip/tpu
LEDGER_LIMIT_SHREDS="50000000"   # limited ledger size
PROM_PORT=9101
PROM_POLL_INTERVAL=5
PROM_CORS_ALLOW_ORIGIN="${PROM_CORS_ALLOW_ORIGIN:-*}"
VALIDATOR_USER="ubuntu"
INSTALL_ROOT="/home/$VALIDATOR_USER/.local/share/solana/install"
RELEASE_DIR="$INSTALL_ROOT/releases/${SOLANA_VERSION}"
ACTIVE_DIR="$INSTALL_ROOT/active_release"
ACTIVE_BIN="$ACTIVE_DIR/bin"
RELEASE_ARCHIVE="/tmp/solana-release-${SOLANA_VERSION}.tar.bz2"

# BASIC SETUP
apt-get update -y
DEBIAN_FRONTEND=noninteractive apt-get install -y \
  curl wget ca-certificates jq build-essential pkg-config libssl-dev bzip2 \
  python3 python3-prometheus-client python3-psutil

# Increase file limits
cat >> /etc/security/limits.conf <<EOF_LIMITS || true
* soft nofile 1000000
* hard nofile 1000000
EOF_LIMITS

# Kernel / network tuning recommended by Solana docs
cat > /etc/sysctl.d/99-solana-validator.conf <<EOF_SYSCTL
net.core.rmem_max = 134217728
net.core.wmem_max = 134217728
net.core.rmem_default = 134217728
net.core.wmem_default = 134217728
net.core.netdev_max_backlog = 500000
net.core.somaxconn = 65535
net.ipv4.udp_mem = 65536 131072 262144
net.ipv4.udp_rmem_min = 16384
net.ipv4.udp_wmem_min = 16384
vm.max_map_count = 1000000
EOF_SYSCTL
sysctl --system

# Create directories
mkdir -p "$LEDGER_DIR" "$ACCOUNTS_DIR" "$(dirname "$IDENTITY")"
chown -R "$VALIDATOR_USER":"$VALIDATOR_USER" /var/solana

# INSTALL SOLANA TOOLCHAIN FROM GITHUB RELEASES
sudo -u "$VALIDATOR_USER" bash -lc "mkdir -p $INSTALL_ROOT/releases && rm -rf $RELEASE_DIR $ACTIVE_DIR"
sudo -u "$VALIDATOR_USER" bash -lc "curl --retry 5 --retry-connrefused --retry-delay 3 -L -o $RELEASE_ARCHIVE $GITHUB_RELEASE_URL"
sudo -u "$VALIDATOR_USER" bash -lc "mkdir -p $RELEASE_DIR && tar -xjf $RELEASE_ARCHIVE -C $RELEASE_DIR --strip-components=1"
sudo -u "$VALIDATOR_USER" bash -lc "ln -sfn $RELEASE_DIR $ACTIVE_DIR"
sudo -u "$VALIDATOR_USER" bash -lc "export PATH=$ACTIVE_BIN:\$PATH && solana --version || true"

# GENERATE IDENTITY
if [ ! -f "$IDENTITY" ]; then
  sudo -u "$VALIDATOR_USER" bash -lc "export PATH=$ACTIVE_BIN:\$PATH && solana-keygen new -o $IDENTITY --no-bip39-passphrase"
fi

PUBKEY=$(sudo -u "$VALIDATOR_USER" bash -lc "export PATH=$ACTIVE_BIN:\$PATH && solana-keygen pubkey $IDENTITY")
echo "Validator identity: $PUBKEY" | tee /root/validator-identity.txt

ENTRYPOINT_ARGS=$(printf '  --entrypoint %s \\\n' "${ENTRYPOINTS[@]}")
KNOWN_VALIDATORS=(
  "Diman2GphWLwECE3swjrAEAJniezpYLxK1edUydiDZau"
  "Cu9Ls6dsTL6cxFHZdStHwVSh1uy2ynXz8qPJMS5FRq86"
  "HLXxkmjb47spcmbbKi3UCfZ2qmFY29t8MN562AEmh2Qh"
  "6XKqyUVUcpe3CNucjF6gk5zonJDqNGvob6kaTy4Ps1U"
  "bkpk9KVsDRfrArzzmkJ9mPEvbXfQxczzQYR3QMGiR8Z"
  "3cZSHGfNdaULpFAvGbWbxpVwzXB4gHdk8NFucPNR5pgA"
  "3cZSHGfNdaULpFAvGbWbxpVwzXB4gHdk8NFucPNR5pgA"
  "7EzbSahSfSjeRexHcNDLDpzHBAGBLjLKtjbmuoQnEtjE"
  "5TGfVQV1S3wHE9hkgGaQkRoPC7xZxiqwMcjQZxVYsf1j"
  "2gDeeRa3mwPPtw1CMWPkEhRWo9v5izNBBfEXanr8uibX"
  "4XspXDcJy3DWZsVdaXrt8pE1xhcLpXDKkhj9XyjmWWNy"
  "qZMH9GWnnBkx7aM1h98iKSv2Lz5N78nwNSocAxDQrbP"
  "8nbE53mcKhy74HLiGZ1q5HRocwiCvgh49csSaHSdtukr"
  "HnwMGBAw5PxaX56eSYc969MorEy2NzEMPLkmBkdnJmeq"
)
KNOWN_VALIDATOR_ARGS=$(printf '  --known-validator %s \\\n' "${KNOWN_VALIDATORS[@]}")

# PROMETHEUS EXPORTER (sidecar script that polls RPC and exposes metrics)
cat > /usr/local/bin/solana-prom-exporter.py <<'EOF_PROM'
#!/usr/bin/env python3
import json
import math
import os
import time
from typing import Optional, Tuple
from urllib import request

import psutil
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import threading
from prometheus_client import Gauge, REGISTRY
from prometheus_client.exposition import CONTENT_TYPE_LATEST, generate_latest

RPC_URL = os.environ.get("SOLANA_RPC", "http://127.0.0.1:8899")
REFERENCE_RPC = os.environ.get("REFERENCE_RPC", "")
LEDGER_PATH = os.environ.get("LEDGER_DIR", "/var/solana/ledger")
VOTE_ACCOUNT = os.environ.get("VOTE_ACCOUNT", "")
VALIDATOR_ID = os.environ.get("VALIDATOR_ID", "")
POLL_INTERVAL = float(os.environ.get("POLL_INTERVAL", "5"))
PROM_CORS_ALLOW_ORIGIN = os.environ.get("PROM_CORS_ALLOW_ORIGIN", "*")

LABELLED = bool(VALIDATOR_ID)


def make_gauge(name: str, description: str) -> Gauge:
    if LABELLED:
        return Gauge(name, description, ["id"])
    return Gauge(name, description)


slot_gauge = make_gauge("solana_validator_slot", "Current slot reported by RPC")
block_height_gauge = make_gauge("solana_validator_block_height", "Current block height")
health_gauge = make_gauge("solana_validator_health_ok", "Validator health (1 ok, 0 otherwise)")
latency_gauge = make_gauge("solana_validator_rpc_latency_ms", "RPC latency in milliseconds")
slot_lag_gauge = make_gauge("validator_slot_lag", "Current slot lag vs reference RPC")
vote_success_gauge = make_gauge("validator_vote_success_rate", "Vote success rate for this validator")
cpu_usage_gauge = make_gauge("validator_cpu_usage", "Validator CPU usage fraction")
ram_usage_gauge = make_gauge("validator_ram_usage_gb", "Validator RAM usage in gigabytes")
disk_usage_gauge = make_gauge("validator_disk_usage_pct", "Validator disk usage percentage")
rpc_qps_gauge = make_gauge("validator_rpc_qps", "Validator RPC queries per second")
rpc_error_rate_gauge = make_gauge("validator_rpc_error_rate", "Validator RPC error rate fraction")
metrics_ts_gauge = make_gauge("validator_metrics_timestamp", "Timestamp of metrics generation")


class CORSMetricsHandler(BaseHTTPRequestHandler):
    def _set_cors_headers(self) -> None:
        self.send_header("Access-Control-Allow-Origin", PROM_CORS_ALLOW_ORIGIN)
        self.send_header("Access-Control-Allow-Methods", "GET,OPTIONS")
        self.send_header("Access-Control-Allow-Headers", "Content-Type")

    def do_OPTIONS(self):  # noqa: N802 (stdlib hook)
        self.send_response(204)
        self._set_cors_headers()
        self.end_headers()

    def do_GET(self):  # noqa: N802 (stdlib hook)
        if self.path not in ("/metrics", "/metrics/"):
            body = b"not found"
            self.send_response(404)
            self.send_header("Content-Type", "text/plain")
            self.send_header("Content-Length", str(len(body)))
            self._set_cors_headers()
            self.end_headers()
            self.wfile.write(body)
            return
        output = generate_latest(REGISTRY)
        self.send_response(200)
        self.send_header("Content-Type", CONTENT_TYPE_LATEST)
        self.send_header("Content-Length", str(len(output)))
        self._set_cors_headers()
        self.end_headers()
        self.wfile.write(output)

    def log_message(self, fmt, *args):  # noqa: D401
        """Silence default stdout logging."""
        return


def start_metrics_server(port: int) -> None:
    server = ThreadingHTTPServer(("0.0.0.0", port), CORSMetricsHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()


def set_value(gauge: Gauge, value: Optional[float]) -> None:
    target = value if value is not None else math.nan
    if LABELLED:
        gauge.labels(id=VALIDATOR_ID).set(target)
    else:
        gauge.set(target)


def rpc_call(
    url: str,
    method: str,
    params=None,
    *,
    timeout: int = 10,
    track_latency: bool = False,
) -> any:
    body = json.dumps(
        {
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params or [],
        }
    ).encode("utf-8")
    req = request.Request(url, data=body, headers={"Content-Type": "application/json"})
    start = time.time()
    with request.urlopen(req, timeout=timeout) as resp:
        payload = json.loads(resp.read().decode("utf-8"))
    if track_latency:
        set_value(latency_gauge, (time.time() - start) * 1000.0)
    if "result" not in payload:
        raise RuntimeError(f"RPC error: {payload}")
    return payload["result"]


def fetch_slot() -> Optional[int]:
    try:
        return rpc_call(RPC_URL, "getSlot", track_latency=True)
    except Exception:
        return None


def fetch_block_height() -> Optional[int]:
    try:
        return rpc_call(RPC_URL, "getBlockHeight")
    except Exception:
        return None


def fetch_health() -> float:
    try:
        health = rpc_call(RPC_URL, "getHealth")
        return 1.0 if health == "ok" else 0.0
    except Exception:
        return 0.0


def compute_slot_lag(local_slot: Optional[int]) -> Optional[float]:
    if not REFERENCE_RPC or local_slot is None:
        return None
    try:
        cluster_slot = rpc_call(REFERENCE_RPC, "getSlot")
    except Exception:
        return None
    return float(max(cluster_slot - local_slot, 0))


def compute_vote_success_rate() -> Optional[float]:
    if not VOTE_ACCOUNT:
        return None
    try:
        vote_accounts = rpc_call(
            RPC_URL,
            "getVoteAccounts",
            [{"votePubkey": VOTE_ACCOUNT}],
        )
        epoch_info = rpc_call(RPC_URL, "getEpochInfo")
    except Exception:
        return None

    slots_in_epoch = epoch_info.get("slotsInEpoch")
    if not slots_in_epoch:
        return None

    accounts = (vote_accounts.get("current") or []) + (vote_accounts.get("delinquent") or [])
    target = None
    for entry in accounts:
        if entry.get("votePubkey") == VOTE_ACCOUNT:
            target = entry
            break
    if target is None:
        return None

    credits = target.get("epochCredits") or []
    if not credits:
        return None
    latest = credits[-1]
    if len(latest) < 3:
        return None
    earned = latest[1] - latest[2]
    if earned <= 0:
        return 0.0
    return max(0.0, min(float(earned) / float(slots_in_epoch), 1.0))


def compute_cpu_usage() -> float:
    return psutil.cpu_percent(interval=None) / 100.0


def compute_ram_usage() -> float:
    mem = psutil.virtual_memory()
    return mem.used / (1024 ** 3)


def compute_disk_usage() -> Optional[float]:
    path = LEDGER_PATH if os.path.isdir(LEDGER_PATH) else "/"
    try:
        return psutil.disk_usage(path).percent
    except Exception:
        return None


def compute_rpc_throughput() -> Tuple[Optional[float], Optional[float]]:
    try:
        samples = rpc_call(RPC_URL, "getRecentPerformanceSamples", [1])
    except Exception:
        return None, None
    if not isinstance(samples, list) or not samples:
        return None, None
    sample = samples[0]
    txs = sample.get("numTransactions")
    period = sample.get("samplePeriodSecs") or 0
    errors = sample.get("numTransactionErrors", 0)
    qps = None
    err_rate = None
    if txs and period:
        qps = float(txs) / float(period)
    if errors is not None and txs:
        err_rate = max(0.0, min(float(errors) / float(txs), 1.0))
    elif errors == 0:
        err_rate = 0.0
    return qps, err_rate


def collect_once():
    local_slot = fetch_slot()
    set_value(slot_gauge, local_slot)
    set_value(block_height_gauge, fetch_block_height())
    set_value(health_gauge, fetch_health())
    set_value(slot_lag_gauge, compute_slot_lag(local_slot))
    set_value(vote_success_gauge, compute_vote_success_rate())
    set_value(cpu_usage_gauge, compute_cpu_usage())
    set_value(ram_usage_gauge, compute_ram_usage())
    set_value(disk_usage_gauge, compute_disk_usage())
    qps, err_rate = compute_rpc_throughput()
    set_value(rpc_qps_gauge, qps)
    set_value(rpc_error_rate_gauge, err_rate)
    set_value(metrics_ts_gauge, time.time())


def main():
    psutil.cpu_percent(interval=None)
    start_metrics_server(int(os.environ.get("PROM_PORT", "9101")))
    while True:
        collect_once()
        time.sleep(POLL_INTERVAL)


if __name__ == "__main__":
    main()
EOF_PROM
chmod +x /usr/local/bin/solana-prom-exporter.py

cat > /etc/systemd/system/solana-prometheus-exporter.service <<EOF_PROM_SERVICE
[Unit]
Description=Solana Validator Prometheus Exporter
After=network-online.target solana-validator.service
Requires=solana-validator.service

[Service]
User=$VALIDATOR_USER
Environment=SOLANA_RPC=http://127.0.0.1:$RPC_PORT
Environment=PROM_PORT=$PROM_PORT
Environment=POLL_INTERVAL=$PROM_POLL_INTERVAL
Environment=REFERENCE_RPC=https://api.mainnet-beta.solana.com
Environment=LEDGER_DIR=$LEDGER_DIR
Environment=VALIDATOR_ID=$PUBKEY
Environment=VOTE_ACCOUNT=
Environment=PROM_CORS_ALLOW_ORIGIN=$PROM_CORS_ALLOW_ORIGIN
ExecStart=/usr/bin/env python3 /usr/local/bin/solana-prom-exporter.py
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF_PROM_SERVICE

# CREATE SYSTEMD SERVICE
cat > /etc/systemd/system/solana-validator.service <<EOF_SERVICE
[Unit]
Description=Solana Non-Voting Validator (mainnet-beta)
After=network-online.target
Wants=network-online.target

[Service]
User=$VALIDATOR_USER
LimitNOFILE=1000000
Environment=PATH=$ACTIVE_BIN:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
ExecStart=$ACTIVE_BIN/solana-validator \\
  --identity $IDENTITY \\
  --ledger $LEDGER_DIR \\
  --accounts $ACCOUNTS_DIR \\
  --rpc-port $RPC_PORT \\
  --rpc-bind-address $RPC_BIND \\
  --dynamic-port-range $DYNAMIC_PORT_RANGE \\
  $ENTRYPOINT_ARGS
  --expected-genesis-hash $GENESIS_HASH \\
  $KNOWN_VALIDATOR_ARGS
  --no-voting \\
  --no-port-check \\
  --limit-ledger-size $LEDGER_LIMIT_SHREDS \\
  --log $LOG_FILE
StandardOutput=journal
StandardError=journal
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF_SERVICE

# Enable & start
systemctl daemon-reload
systemctl enable solana-validator solana-prometheus-exporter
systemctl start solana-validator solana-prometheus-exporter
