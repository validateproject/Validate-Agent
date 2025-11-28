import type { PrometheusMetric } from "./types";

const DEFAULT_PROM_URL = "http://3.79.61.58:9101/metrics";

const PROMETHEUS_URL =
  import.meta.env.VITE_PROM_METRICS_URL?.trim() || DEFAULT_PROM_URL;

export async function fetchPrometheusMetrics(): Promise<PrometheusMetric[]> {
  const res = await fetch(PROMETHEUS_URL, {
    cache: "no-store"
  });
  if (!res.ok) {
    throw new Error(`Metrics request failed: ${res.status}`);
  }
  const body = await res.text();
  return parsePrometheusBody(body);
}

const METRIC_LINE =
  /^([A-Za-z_:][\w:]*)(\{[^}]*\})?\s+(-?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?|NaN|Inf|-Inf)$/;

function parsePrometheusBody(body: string): PrometheusMetric[] {
  const metrics: PrometheusMetric[] = [];
  const lines = body.split("\n");

  for (const rawLine of lines) {
    const line = rawLine.trim();
    if (!line || line.startsWith("#")) {
      continue;
    }

    const match = line.match(METRIC_LINE);
    if (!match) {
      continue;
    }

    const [, name, labelGroup, valueToken] = match;
    metrics.push({
      name,
      labels: labelGroup ? parseLabels(labelGroup) : {},
      value: parseValue(valueToken)
    });
  }

  return metrics;
}

function parseLabels(group: string): Record<string, string> {
  const labels: Record<string, string> = {};
  const content = group.slice(1, -1);
  if (!content) {
    return labels;
  }

  const parts = content.split(/,(?=(?:[^"]*"[^"]*")*[^"]*$)/);
  for (const part of parts) {
    const [key, rawValue] = part.split("=");
    if (!key || rawValue == null) {
      continue;
    }
    const value = rawValue.replace(/^"/, "").replace(/"$/, "");
    labels[key.trim()] = value.replace(/\\"/g, '"').replace(/\\\\/g, "\\");
  }
  return labels;
}

function parseValue(token: string): number {
  if (token === "NaN") return Number.NaN;
  if (token === "Inf" || token === "+Inf") return Number.POSITIVE_INFINITY;
  if (token === "-Inf") return Number.NEGATIVE_INFINITY;
  return Number(token);
}

