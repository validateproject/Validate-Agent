import { useEffect, useMemo, useState } from "react";
import { fetchActions, fetchValidators } from "./api";
import type { ValidatorSummary } from "./types";
import "./index.css";

const REFRESH_INTERVAL_MS = 5000;

export default function App() {
  const [validators, setValidators] = useState<ValidatorSummary[]>([]);
  const [pendingActions, setPendingActions] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [isLoading, setIsLoading] = useState(true);

  useEffect(() => {
    let mounted = true;

    async function load() {
      try {
        const [validatorsRes, actionsRes] = await Promise.all([
          fetchValidators(),
          fetchActions()
        ]);
        if (!mounted) {
          return;
        }
        setValidators(validatorsRes.validators);
        setPendingActions(actionsRes.pending);
        setError(null);
      } catch (err) {
        console.error(err);
        if (mounted) {
          setError(
            err instanceof Error
              ? err.message
              : "Failed to load data from the agent API."
          );
        }
      } finally {
        if (mounted) {
          setIsLoading(false);
        }
      }
    }

    load();
    const id = setInterval(load, REFRESH_INTERVAL_MS);
    return () => {
      mounted = false;
      clearInterval(id);
    };
  }, []);

  const sortedValidators = useMemo(
    () =>
      [...validators].sort((a, b) => {
        const ar = a.risk_score ?? 0;
        const br = b.risk_score ?? 0;
        return br - ar;
      }),
    [validators]
  );

  return (
    <div className="page">
      <header className="header">
        <div>
          <p className="eyebrow">Validator Copilot</p>
          <h1>Agent Dashboard</h1>
          <p className="subdued">
            Live metrics pulled from Redis plus the pending action queue.
          </p>
        </div>
        <div className="stat-card">
          <p className="stat-label">Pending actions</p>
          <p className="stat-value">{pendingActions}</p>
        </div>
      </header>

      {error && <div className="banner warning">{error}</div>}
      {isLoading && !error ? (
        <div className="banner info">Loading latest data…</div>
      ) : null}

      <div className="table-wrapper">
        <table>
          <thead>
            <tr>
              <th>Validator</th>
              <th>Host</th>
              <th>Status</th>
              <th>Risk</th>
              <th>Slot Lag</th>
              <th>CPU</th>
              <th>RAM</th>
              <th>Disk</th>
              <th>Vote OK</th>
              <th>Last Updated</th>
            </tr>
          </thead>
          <tbody>
            {sortedValidators.map((validator) => (
              <tr key={validator.id}>
                <td>{validator.id}</td>
                <td>{validator.host}</td>
                <td>
                  <StatusBadge status={validator.status} />
                </td>
                <td>{formatRisk(validator.risk_score)}</td>
                <td>{formatNumber(validator.metrics?.slot_lag)}</td>
                <td>{formatPercent(validator.metrics?.cpu_usage)}</td>
                <td>{formatNumber(validator.metrics?.ram_usage_gb, " GB")}</td>
                <td>
                  {formatPercent(validator.metrics?.disk_usage_pct, {
                    alreadyPercent: true
                  })}
                </td>
                <td>{formatPercent(validator.metrics?.vote_success_rate)}</td>
                <td>{formatTimestamp(validator.metrics?.last_updated)}</td>
              </tr>
            ))}
            {sortedValidators.length === 0 && (
              <tr>
                <td colSpan={10} className="subdued">
                  No validators configured yet.
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
    </div>
  );
}

function StatusBadge({ status }: { status: string }) {
  if (status === "ok") {
    return <span className="badge success">OK</span>;
  }
  if (status === "no_data") {
    return <span className="badge muted">No data</span>;
  }
  if (status === "invalid_metrics") {
    return <span className="badge warning">Invalid metrics</span>;
  }
  return <span className="badge danger">{status}</span>;
}

function formatRisk(value: number | null | undefined) {
  if (value == null) return "–";
  return value.toFixed(2);
}

function formatNumber(value: number | null | undefined, suffix = "") {
  if (value == null || Number.isNaN(value)) return "–";
  return `${value.toFixed(0)}${suffix}`;
}

function formatPercent(
  value: number | null | undefined,
  options?: { alreadyPercent?: boolean }
) {
  if (value == null || Number.isNaN(value)) return "–";
  const percent = options?.alreadyPercent ? value : value * 100;
  return `${percent.toFixed(1)}%`;
}

function formatTimestamp(value: number | null | undefined) {
  if (value == null || value === 0) return "–";
  const date = new Date(value * 1000);
  return date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

