import { useEffect, useMemo, useRef, useState } from "react";
import { fetchPrometheusMetrics } from "./api";
import type { LogSample, MetricSnapshot, PrometheusMetric } from "./types";
import "./index.css";

const REFRESH_INTERVAL_MS = 1000;
const RANDOM_LOG_INTERVAL_MS = 3200;
const MAX_LOG_ENTRIES = 80;
const SLOTS_PER_EPOCH = 432000;

type LogEntry = LogSample & {
  id: string;
  timestamp: string;
};

const INITIAL_LOGS: LogSample[] = [
  {
    level: "info",
    tag: "identity",
    message: "node pubkey: 7e3K..LGZB · shred_version=54762 · gossip=8900"
  },
  {
    level: "info",
    tag: "bank",
    message: "slot 221004930 root 221004901 | txs=512 | leader=35ms | stake 1.02%"
  },
  {
    level: "warn",
    tag: "rpc",
    message:
      "lagged by 3 slots behind root, backfilling historical accounts (since 221004897)"
  },
  {
    level: "info",
    tag: "consensus",
    message:
      "new fork choice: slot 221004932 (confirmed) | ancestor distance 2 | lockouts ok"
  },
  {
    level: "error",
    tag: "vote",
    message:
      "transaction rejected: lockout conflict on slot 221004927 · retrying after delay"
  },
  {
    level: "info",
    tag: "snapshot",
    message:
      "verified incremental snapshot (base=221003200, full=221000000) · 1.8s"
  },
  {
    level: "info",
    tag: "ledger",
    message:
      "completed shred insert | slot 221004934 | batches=3 | disk=11.3ms | lz4"
  },
  {
    level: "warn",
    tag: "quic",
    message: "peer 8p9d..B3kv slow ack path · rtt=118ms · inflight=42 | throttling"
  },
  {
    level: "info",
    tag: "rpc",
    message: "slot 221004934 confirmed · subscriptions=182 · fanout ok"
  },
  {
    level: "info",
    tag: "bank",
    message: "slot 221004935 root 221004904 | txs=486 | leader=36ms | cost_model=ok"
  }
];

const RANDOM_SAMPLES: LogSample[] = [
  {
    level: "info",
    tag: "consensus",
    message: "accepted tower vote for slot 221004936 | lockouts updated"
  },
  {
    level: "info",
    tag: "bank",
    message: "slot 221004937 root 221004905 | txs=498 | leader=32ms"
  },
  {
    level: "warn",
    tag: "rpc",
    message: "client subscription backlog 12 msgs (ws) · clearing"
  },
  {
    level: "info",
    tag: "ledger",
    message: "shreds: insert=2.8ms verify=3.1ms | idx=221004937"
  },
  {
    level: "error",
    tag: "vote",
    message: "vote send failed: cluster busy, retry queued (slot 221004936)"
  },
  {
    level: "info",
    tag: "accounts",
    message: "cleaned 13k accounts | reclaimed 82 MB"
  },
  {
    level: "warn",
    tag: "quic",
    message: "peer 3hSk..2fwc congestion window reduced · loss=2.1%"
  },
  {
    level: "info",
    tag: "snapshot",
    message: "pruning old snapshots > 221001600 | kept=2"
  },
  {
    level: "info",
    tag: "rpc",
    message: "blockstore roots synced (221004902-221004937)"
  },
  {
    level: "info",
    tag: "gossip",
    message: "validators: 2188 | stakes observed: 24.32B"
  }
];

export default function App() {
  const [snapshot, setSnapshot] = useState<MetricSnapshot | null>(null);
  const [lastUpdated, setLastUpdated] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [logs, setLogs] = useState<LogEntry[]>(() =>
    INITIAL_LOGS.map((sample, idx) =>
      createLogEntry(sample, Date.now() - (INITIAL_LOGS.length - idx) * 800)
    )
  );

  const logRef = useRef<HTMLDivElement | null>(null);
  const fingerprintRef = useRef<string>("");

  useEffect(() => {
    logRef.current?.scrollTo({
      top: logRef.current.scrollHeight,
      behavior: "smooth"
    });
  }, [logs]);

  useEffect(() => {
    let mounted = true;

    async function load() {
      try {
        const metrics = await fetchPrometheusMetrics();
        if (!mounted) return;
        setSnapshot(buildSnapshot(metrics));
        setLastUpdated(Date.now());
        setError(null);
      } catch (err) {
        if (!mounted) return;
        console.error(err);
        setError(
          err instanceof Error
            ? err.message
            : "Unable to pull the Prometheus metrics feed."
        );
      }
    }

    load();
    const id = setInterval(load, REFRESH_INTERVAL_MS);
    return () => {
      mounted = false;
      clearInterval(id);
    };
  }, []);

  useEffect(() => {
    const id = setInterval(() => {
      const sample =
        RANDOM_SAMPLES[Math.floor(Math.random() * RANDOM_SAMPLES.length)];
      setLogs((prev) => appendLog(prev, sample));
    }, RANDOM_LOG_INTERVAL_MS);
    return () => clearInterval(id);
  }, []);

  useEffect(() => {
    if (!snapshot) return;
    const fingerprint = [
      snapshot.slot ?? "-",
      snapshot.cpuUsage ?? "-",
      snapshot.voteSuccessRate ?? "-"
    ].join(":");

    if (fingerprintRef.current === fingerprint) {
      return;
    }
    fingerprintRef.current = fingerprint;

    const parts = [
      snapshot.slot ? `slot ${formatNumber(snapshot.slot, 0)}` : null,
      snapshot.slotLag != null
        ? `lag ${formatNumber(snapshot.slotLag, 0)} slots`
        : null,
      snapshot.cpuUsage != null
        ? `cpu ${formatPercent(snapshot.cpuUsage)}`
        : null,
      snapshot.voteSuccessRate != null
        ? `vote ${formatPercent(snapshot.voteSuccessRate)}`
        : null
    ].filter(Boolean);

    if (parts.length === 0) {
      return;
    }

    setLogs((prev) =>
      appendLog(prev, {
        level: "info",
        tag: "metrics",
        message: parts.join(" | ")
      })
    );
  }, [snapshot]);

  const slot = snapshot?.slot ?? null;
  const epoch =
    slot != null ? Math.max(Math.floor(slot / SLOTS_PER_EPOCH), 0) : null;
  const healthOk = snapshot?.healthOk === 1;

  const footerDetails = useMemo(() => {
    const uptimeSeconds =
      snapshot?.startTime != null
        ? Math.max(0, Date.now() / 1000 - snapshot.startTime)
        : null;
    return {
      uptime: formatDuration(uptimeSeconds),
      lastBlock:
        snapshot?.blockHeight != null
          ? formatNumber(snapshot.blockHeight, 0)
          : slot != null
          ? formatNumber(slot, 0)
          : "—",
      latency: formatFreshness(snapshot?.timestamp ?? null),
      openFds: snapshot?.processOpenFds ?? null
    };
  }, [snapshot, slot]);

  return (
    <div className="page">
      <main className="frame">
        <header className="header">
          <div><img src="/logo.png" alt="Solana" width={42} height={42} /></div>
          <div className="titles">
            <h1>Validate Protocol Logs</h1>
            <span>Live Feed</span>
          </div>
          <div className={`status ${healthOk ? "ok" : "warn"}`}>
            {healthOk ? "stream healthy" : "stream degraded"}
            {lastUpdated && (
              <span className="status-meta">
                {` · updated ${formatRelativeTime(lastUpdated)}`}
              </span>
            )}
          </div>
        </header>

        <section className="controls">
          <div className="pill live">
            <div className="dot" />
            live tail
          </div>
          <div className="pill info">
            <div className="dot" />
            info
          </div>
          <div className="pill warn">
            <div className="dot" />
            warn
          </div>
          <div className="pill error">
            <div className="dot" />
            error
          </div>
          <span>
            slot: <strong>{formatNumber(slot, 0)}</strong>
          </span>
          <span>
            epoch: <strong>{formatNumber(epoch, 0)}</strong>
          </span>
          <span className="flex-spacer" />
          <button className="pill action" type="button" disabled>
            fullscreen
          </button>
        </section>

        {error && <div className="banner warning">{error}</div>}

        <section className="metrics-row">
          <MetricPill label="slot lag" value={formatNumber(snapshot?.slotLag, 0)} />
          <MetricPill
            label="vote success"
            value={formatPercent(snapshot?.voteSuccessRate)}
          />
          <MetricPill
            label="cpu usage"
            value={formatPercent(snapshot?.cpuUsage)}
          />
          <MetricPill
            label="ram"
            value={
              snapshot?.ramUsageGb != null
                ? `${snapshot.ramUsageGb.toFixed(3)} GB`
                : "—"
            }
          />
          <MetricPill
            label="disk"
            value={
              snapshot?.diskUsagePct != null
                ? `${snapshot.diskUsagePct.toFixed(1)}%`
                : "—"
            }
          />
        </section>

        <section className="console">
          <div className="console-header">
            <span>validator</span> rpc06.mainnet.solana
            <span className="console-path">tail -f /var/log/solana/validator.log</span>
          </div>
          <div className="log-stream" ref={logRef}>
            {logs.map((log) => (
              <LogLine key={log.id} entry={log} />
            ))}
          </div>
        </section>

        <section className="system-metrics">
          <MetricCard
            label="process cpu"
            value={
              snapshot?.cpuUsage != null
                ? formatPercent(snapshot.cpuUsage)
                : "—"
            }
            detail={
              snapshot?.processCpuSeconds != null
                ? `${snapshot.processCpuSeconds.toFixed(2)}s total`
                : "waiting for samples"
            }
          />
          <MetricCard
            label="memory"
            value={formatBytes(snapshot?.ramBytes)}
            detail={
              snapshot?.virtualMemBytes != null
                ? `${formatBytes(snapshot.virtualMemBytes)} virtual`
                : "virtual unknown"
            }
          />
          <MetricCard
            label="open fds"
            value={
              snapshot?.processOpenFds != null
                ? snapshot.processOpenFds.toString()
                : "—"
            }
            detail="descriptor usage"
          />
          <MetricCard
            label="gc collections"
            value={formatGc(snapshot?.gcCollections)}
            detail={`objects: ${formatGc(snapshot?.gcObjectsCollected)}`}
          />
        </section>

        <div className="footer">
          <div>
            <strong>tailing</strong> validator.log · uptime{" "}
            <strong>{footerDetails.uptime}</strong>
          </div>
          <div>
            latency <strong>{footerDetails.latency}</strong> · last block{" "}
            <strong>{footerDetails.lastBlock}</strong>
          </div>
        </div>
      </main>
    </div>
  );
}

function LogLine({ entry }: { entry: LogEntry }) {
  return (
    <div className="line">
      <span className="timestamp">{entry.timestamp}</span>
      <span className={`level ${entry.level}`}>{entry.level.toUpperCase()}</span>
      <span className="message">
        <span className="tag">{entry.tag}</span>
        {entry.message}
      </span>
    </div>
  );
}

function MetricPill({ label, value }: { label: string; value: string }) {
  return (
    <div className="pill metric-pill">
      <div className="metric-pill__label">{label}</div>
      <div className="metric-pill__value">{value}</div>
    </div>
  );
}

function MetricCard({
  label,
  value,
  detail
}: {
  label: string;
  value: string;
  detail: string;
}) {
  return (
    <div className="metric-card">
      <p className="metric-label">{label}</p>
      <p className="metric-value">{value}</p>
      <p className="metric-detail">{detail}</p>
    </div>
  );
}

function appendLog(logs: LogEntry[], sample: LogSample) {
  const next = [...logs, createLogEntry(sample)];
  if (next.length > MAX_LOG_ENTRIES) {
    return next.slice(next.length - MAX_LOG_ENTRIES);
  }
  return next;
}

function createLogEntry(sample: LogSample, seed?: number): LogEntry {
  const timestamp = seed ? new Date(seed) : new Date();
  return {
    ...sample,
    id: `${timestamp.getTime()}-${Math.random().toString(36).slice(2, 7)}`,
    timestamp: formatTimestamp(timestamp)
  };
}

function formatTimestamp(date: Date) {
  return `${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(
    date.getSeconds()
  )}.${date.getMilliseconds().toString().padStart(3, "0")}`;
}

function pad(value: number) {
  return value.toString().padStart(2, "0");
}

function formatNumber(value: number | null | undefined, digits = 0) {
  if (value == null || Number.isNaN(value)) {
    return "—";
  }
  return value.toLocaleString(undefined, {
    minimumFractionDigits: digits,
    maximumFractionDigits: digits
  });
}

function formatPercent(value: number | null | undefined) {
  if (value == null || Number.isNaN(value)) {
    return "—";
  }
  return `${(value * 100).toFixed(1)}%`;
}

function formatBytes(value: number | null | undefined) {
  if (value == null || Number.isNaN(value)) {
    return "—";
  }
  const units = ["B", "KB", "MB", "GB", "TB"];
  let idx = 0;
  let current = value;
  while (current >= 1024 && idx < units.length - 1) {
    current /= 1024;
    idx += 1;
  }
  return `${current.toFixed(idx === 0 ? 0 : 2)} ${units[idx]}`;
}

function formatDuration(value: number | null | undefined) {
  if (value == null || Number.isNaN(value)) {
    return "—";
  }
  const hours = Math.floor(value / 3600);
  const minutes = Math.floor((value % 3600) / 60);
  if (hours > 0) {
    return `${hours}h ${minutes}m`;
  }
  if (minutes > 0) {
    return `${minutes}m`;
  }
  return `${Math.max(1, Math.floor(value))}s`;
}

function formatRelativeTime(timestamp: number) {
  const diff = Date.now() - timestamp;
  if (diff < 2000) return "just now";
  if (diff < 60000) return `${Math.round(diff / 1000)}s ago`;
  return `${Math.round(diff / 60000)}m ago`;
}

function formatFreshness(timestamp: number | null) {
  if (!timestamp) {
    return "—";
  }
  const diffMs = Date.now() - timestamp * 1000;
  if (diffMs < 0) return "0 ms";
  if (diffMs < 1000) return `${diffMs.toFixed(0)} ms`;
  if (diffMs < 60000) return `${(diffMs / 1000).toFixed(1)} s`;
  return `${Math.round(diffMs / 60000)} m`;
}

function formatGc(values: Record<string, number> | undefined | null) {
  if (!values || Object.keys(values).length === 0) {
    return "—";
  }
  return Object.entries(values)
    .map(([gen, val]) => `g${gen}:${Number.isFinite(val) ? val : 0}`)
    .join(" · ");
}

function buildSnapshot(metrics: PrometheusMetric[]): MetricSnapshot {
  return {
    slot: pickMetric(metrics, "solana_validator_slot"),
    blockHeight: pickMetric(metrics, "solana_validator_block_height"),
    healthOk: pickMetric(metrics, "solana_validator_health_ok"),
    slotLag: pickMetric(metrics, "validator_slot_lag"),
    voteSuccessRate: pickMetric(metrics, "validator_vote_success_rate"),
    cpuUsage: pickMetric(metrics, "validator_cpu_usage"),
    ramUsageGb: pickMetric(metrics, "validator_ram_usage_gb"),
    diskUsagePct: pickMetric(metrics, "validator_disk_usage_pct"),
    ramBytes: pickMetric(metrics, "process_resident_memory_bytes"),
    virtualMemBytes: pickMetric(metrics, "process_virtual_memory_bytes"),
    processCpuSeconds: pickMetric(metrics, "process_cpu_seconds_total"),
    processOpenFds: pickMetric(metrics, "process_open_fds"),
    gcCollections: pickMetricGroup(metrics, "python_gc_collections_total", "generation"),
    gcObjectsCollected: pickMetricGroup(
      metrics,
      "python_gc_objects_collected_total",
      "generation"
    ),
    timestamp: pickMetric(metrics, "validator_metrics_timestamp"),
    startTime: pickMetric(metrics, "process_start_time_seconds")
  };
}

function pickMetric(
  metrics: PrometheusMetric[],
  name: string,
  predicate?: (labels: Record<string, string>) => boolean
) {
  const match = metrics.find(
    (metric) => metric.name === name && (!predicate || predicate(metric.labels))
  );
  if (!match) {
    return null;
  }
  return Number.isFinite(match.value) ? match.value : null;
}

function pickMetricGroup(
  metrics: PrometheusMetric[],
  name: string,
  labelKey: string
) {
  return metrics
    .filter((metric) => metric.name === name && metric.labels[labelKey] != null)
    .reduce<Record<string, number>>((acc, metric) => {
      const key = metric.labels[labelKey];
      if (!key) {
        return acc;
      }
      acc[key] = Number.isFinite(metric.value) ? metric.value : Number.NaN;
      return acc;
    }, {});
}

