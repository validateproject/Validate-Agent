import type { ActionsResponse, ValidatorsResponse } from "./types";

const API_BASE =
  import.meta.env.VITE_AGENT_API?.replace(/\/$/, "") || "http://localhost:3000";

async function fetchJson<T>(path: string): Promise<T> {
  const res = await fetch(`${API_BASE}${path}`);
  if (!res.ok) {
    throw new Error(`Request failed: ${res.status}`);
  }
  return (await res.json()) as T;
}

export function fetchValidators() {
  return fetchJson<ValidatorsResponse>("/api/validators");
}

export function fetchActions() {
  return fetchJson<ActionsResponse>("/api/actions");
}

