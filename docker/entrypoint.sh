#!/bin/sh
set -eu

if [ $# -eq 0 ]; then
  echo "No command provided to entrypoint." >&2
  exit 1
fi

exec "$@"

