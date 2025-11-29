#!/usr/bin/env bash

set -euxo pipefail

# CONFIG
AGAVE_VERSION="${AGAVE_VERSION:-__AGAVE_VERSION__}"
CLUSTER="mainnet-beta"
LEDGER_MOUNT="/var/solana"
LEDGER_DIR="$LEDGER_MOUNT/ledger"
ACCOUNTS_DIR="$LEDGER_MOUNT/accounts"
IDENTITY="$LEDGER_MOUNT/validator-keypair.json"
VOTE_ACCOUNT="$LEDGER_MOUNT/vote-account-keypair.json"
LOG_FILE="$LEDGER_MOUNT/validator.log"
GENESIS_HASH="5eykt4UsFv8P8NJdTREpY1vzqKqZKvdpKuc147dw2N9d"
RPC_PORT=8899
DYNAMIC_PORT_RANGE="8000-8025"
LEDGER_DEVICE_ALIAS="/dev/sdf" # Terraform requests this name; Nitro exposes /dev/nvme1n1
ROOT_BLOCK_DEVICE=$(lsblk -no PKNAME "$(findmnt -n -o SOURCE /)" 2>/dev/null || true)
METRICS_ENV="host=https://metrics.solana.com:8086,db=mainnet-beta,u=mainnet-beta_write,p=password"
VALIDATOR_USER="ubuntu"
INSTALL_ROOT="/home/$VALIDATOR_USER/.local/share/agave/install"
RELEASE_DIR="$INSTALL_ROOT/releases/${AGAVE_VERSION}"
ACTIVE_DIR="$INSTALL_ROOT/active_release"
ACTIVE_BIN="$ACTIVE_DIR/bin"
SRC_DIR="/home/$VALIDATOR_USER/agave-src"
AGAVE_REPO="https://github.com/anza-xyz/agave.git"
PROMETHEUS_ENABLED="${PROMETHEUS_ENABLED:-true}"
PROMETHEUS_BIND="${PROMETHEUS_BIND:-0.0.0.0:9103}"

# BASIC SETUP
apt-get update -y
DEBIAN_FRONTEND=noninteractive apt-get install -y \
  curl wget ca-certificates jq git build-essential pkg-config libssl-dev libudev-dev llvm clang libclang-dev protobuf-compiler bzip2 parted

# PREP LEDGER DISK
detect_ledger_device() {
  local alias="$1"
  local root_device=""
  if [ -n "$ROOT_BLOCK_DEVICE" ]; then
    root_device="/dev/$ROOT_BLOCK_DEVICE"
  else
    root_device="/dev/nvme0n1"
  fi
  if [ -b "$alias" ]; then
    readlink -f "$alias" || echo "$alias"
    return
  fi

  # Fallback: locate the first NVMe disk that is not the root device
  local candidate
  for candidate in /dev/nvme*n1; do
    [ -e "$candidate" ] || continue
    [ "$candidate" = "$root_device" ] && continue
    if [ -b "$candidate" ]; then
      echo "$candidate"
      return
    fi
  done

  echo ""
}

wait_for_ledger_device() {
  local alias="$1"
  local device=""
  local attempts=30

  for i in $(seq 1 $attempts); do
    device=$(detect_ledger_device "$alias")
    if [ -n "$device" ]; then
      echo "$device"
      return
    fi
    echo "Ledger device $alias not ready (attempt $i/$attempts); retrying in 5s"
    sleep 5
  done

  echo ""
}

setup_ledger_disk() {
  local alias="$1"
  local device
  device=$(wait_for_ledger_device "$alias")

  if [ -z "$device" ]; then
    echo "Ledger device $alias not present; using root disk for /var/solana"
    mkdir -p "$LEDGER_MOUNT"
    return
  fi

  local part_suffix="1"
  if [[ "$device" =~ [0-9]$ ]]; then
    part_suffix="p1"
  fi
  local part="${device}${part_suffix}"

  if [ ! -b "$part" ]; then
    parted -s "$device" mklabel gpt
    parted -s "$device" mkpart primary ext4 0% 100%
    partprobe "$device"
    udevadm settle || true
    sleep 2
  fi

  local fstype=""
  fstype=$(lsblk -no FSTYPE "$part" 2>/dev/null || true)
  if [ -z "$fstype" ]; then
    mkfs.ext4 -F "$part"
  fi

  mkdir -p "$LEDGER_MOUNT"

  if ! grep -qs "$part $LEDGER_MOUNT" /etc/fstab; then
    local uuid
    uuid=$(blkid -s UUID -o value "$part")
    echo "UUID=$uuid $LEDGER_MOUNT ext4 defaults,nofail 0 2" >> /etc/fstab
  fi

  mount "$LEDGER_MOUNT"
}

setup_ledger_disk "$LEDGER_DEVICE_ALIAS"

cat >> /etc/security/limits.conf <<EOF_LIMITS || true
* soft nofile 1000000
* hard nofile 1000000
EOF_LIMITS

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

mkdir -p "$LEDGER_DIR" "$ACCOUNTS_DIR" "$(dirname "$IDENTITY")"
chown -R "$VALIDATOR_USER":"$VALIDATOR_USER" /var/solana

# INSTALL RUST TOOLCHAIN & BUILD AGAVE
sudo -u "$VALIDATOR_USER" bash -lc '
  set -euxo pipefail
  if ! command -v rustup >/dev/null 2>&1; then
    curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
  fi
  source "$HOME/.cargo/env"
  rustup update stable
  rustup default stable
'

sudo -u "$VALIDATOR_USER" bash -lc "
  set -euxo pipefail
  rm -rf $SRC_DIR
  git clone --branch $AGAVE_VERSION --depth 1 $AGAVE_REPO $SRC_DIR
  mkdir -p $INSTALL_ROOT/releases
  rm -rf $RELEASE_DIR
  mkdir -p $RELEASE_DIR
  cd $SRC_DIR
  source \$HOME/.cargo/env
  ./scripts/cargo-install-all.sh $RELEASE_DIR
  ln -sfn $RELEASE_DIR $ACTIVE_DIR
  export PATH=$ACTIVE_BIN:\$PATH
  solana --version || true
"

PROMETHEUS_SUPPORTED="false"
if sudo -u "$VALIDATOR_USER" bash -lc "export PATH=$ACTIVE_BIN:\$PATH && agave-validator --help 2>&1 | grep -q -- '--prometheus-host'"; then
  PROMETHEUS_SUPPORTED="true"
fi

# GENERATE IDENTITY
if [ ! -f "$IDENTITY" ]; then
  sudo -u "$VALIDATOR_USER" bash -lc "export PATH=$ACTIVE_BIN:\$PATH && solana-keygen new -o $IDENTITY --no-bip39-passphrase"
fi

PUBKEY=$(sudo -u "$VALIDATOR_USER" bash -lc "export PATH=$ACTIVE_BIN:\$PATH && solana-keygen pubkey $IDENTITY")
echo "Validator identity: $PUBKEY" | tee /root/validator-identity.txt

VOTE_ARG="--no-voting"
if [ -f "$VOTE_ACCOUNT" ]; then
  VOTE_ARG="--vote-account $VOTE_ACCOUNT"
else
  echo "Vote account keypair not found at $VOTE_ACCOUNT. Starting in non-voting mode." | tee /root/vote-account-warning.txt
fi

PROMETHEUS_ARG=""
if [ "$PROMETHEUS_ENABLED" = "true" ] && [ "$PROMETHEUS_SUPPORTED" = "true" ]; then
  PROMETHEUS_ARG="  --prometheus-host $PROMETHEUS_BIND \\
"
elif [ "$PROMETHEUS_ENABLED" = "true" ] && [ "$PROMETHEUS_SUPPORTED" = "false" ]; then
  echo "Prometheus flag requested but not supported by agave-validator; skipping" | tee /root/prometheus-unsupported.txt
fi

cat > /etc/systemd/system/agave-validator.service <<EOF_SERVICE
[Unit]
Description=Agave Validator (${CLUSTER})
After=network-online.target
Wants=network-online.target

[Service]
User=$VALIDATOR_USER
LimitNOFILE=1000000
LimitMEMLOCK=infinity
Environment=PATH=$ACTIVE_BIN:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
Environment=SOLANA_METRICS_CONFIG=$METRICS_ENV
ExecStart=$ACTIVE_BIN/agave-validator \\
  --identity $IDENTITY \\
  $VOTE_ARG \\
  --ledger $LEDGER_DIR \\
  --accounts $ACCOUNTS_DIR \\
  --rpc-port $RPC_PORT \\
  --dynamic-port-range $DYNAMIC_PORT_RANGE \\
${PROMETHEUS_ARG}  --entrypoint entrypoint.mainnet-beta.solana.com:8001 \\
  --entrypoint entrypoint.mainnet-beta.solana.com:8001 \\
  --entrypoint entrypoint2.mainnet-beta.solana.com:8001 \\
  --entrypoint entrypoint3.mainnet-beta.solana.com:8001 \\
  --entrypoint entrypoint4.mainnet-beta.solana.com:8001 \\
  --entrypoint entrypoint5.mainnet-beta.solana.com:8001 \\
  --expected-genesis-hash $GENESIS_HASH \\
  --known-validator 7Np41oeYqPefeNQEHSv1UDhYrehxin3NStELsSKCT4K2 \\
  --known-validator GdnSyH3YtwcxFvQrVVJMm1JhTS4QVX7MFsX56uJLUfiZ \\
  --known-validator DE1bawNcRJB9rVm3buyMVfr8mBEoyyu73NBovf2oXJsJ \\
  --known-validator CakcnaRDHka2gXyfbEd2d3xsvkJkqsLw2akB3zsN1D2S \\
  --only-known-rpc \\
  --private-rpc \\
  --wal-recovery-mode skip_any_corrupted_record \\
  --limit-ledger-size \\
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
systemctl enable agave-validator
systemctl start agave-validator
