export interface CostGroup {
  key: string;
  amount: number;
}
export interface CostPeriod {
  period: string;
  total: number;
  groups: CostGroup[];
}
export interface DailyPoint {
  date: string;
  amount: number;
}
export interface OneOff {
  usageType: string;
  amount: number;
}
export interface RunRate {
  runRateMonthly: number;
  oneOffMtd: number;
  oneOffItems: OneOff[];
}
export interface CostData {
  byService: CostPeriod[];
  byAccount: CostPeriod[];
  byRegion: CostPeriod[];
  daily: DailyPoint[];
  forecastNextMonth: number | null;
  runRate: RunRate;
  flags: { uncoveredRegionsWithSpend: string[] };
  generatedAt: string;
}

export interface ResourceRow {
  arn: string;
  type: string;
  region: string;
  service: string;
  name: string;
  /** aws-managed | tooling | app | orphan | unclaimed */
  category: string;
  app: string | null;
  protected: boolean;
  reason: string;
  /** Owning AWS account id, and its org name (or "this account" for the local one). */
  account?: string;
  accountName?: string;
  /** True when its app came from a manual classification override. */
  override?: boolean;
  /** "marked" once flagged for deletion (consumed by the reap tool); absent otherwise. */
  mark?: string | null;
}
/** An org member account the API tried but couldn't inventory (no role / no RE). */
export interface NotIndexed {
  account: string;
  accountName: string;
  reason: string;
}
export interface InventoryData {
  count: number;
  resources: ResourceRow[];
  byRegion: Record<string, number>;
  byApp: Record<string, number>;
  byCategory: Record<string, number>;
  byAccount?: Record<string, number>;
  /** Current-month spend per app key (keys match accordion keys); best-effort. */
  byAppCost?: Record<string, number>;
  flags: { orphans: number; unclaimed: number; marked: number; notIndexed?: NotIndexed[] };
  indexedRegions: string[];
  generatedAt: string;
}

function authHeaders(token: string | undefined): HeadersInit {
  // CloudFront's Lambda OAC owns Authorization (SigV4); the ID token rides in X-Id-Token.
  return token ? { "X-Id-Token": token } : {};
}

async function get<T>(path: string, token: string | undefined): Promise<T> {
  const r = await fetch(path, { headers: authHeaders(token) });
  if (r.status === 401) throw new Error("unauthorized");
  if (!r.ok) throw new Error(`${path} returned ${r.status}`);
  return r.json();
}

export const getCost = (token: string | undefined) => get<CostData>("/api/cost", token);
export const getInventory = (token: string | undefined, refresh = false) =>
  get<InventoryData>(`/api/inventory${refresh ? "?refresh=1" : ""}`, token);

async function post<T>(path: string, token: string | undefined, body: unknown): Promise<T> {
  const r = await fetch(path, {
    method: "POST",
    headers: { ...authHeaders(token), "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  if (r.status === 401) throw new Error("unauthorized");
  if (!r.ok) throw new Error(`${path} returned ${r.status}`);
  return r.json();
}

/** Attribute resources to an app, or pass null to clear the override (back to inferred). */
export const reclassify = (token: string | undefined, arns: string[], app: string | null) =>
  post<{ ok: boolean; count: number }>("/api/inventory/classify", token, { arns, app });

/** Flag resources for deletion (marked=true) or clear the flag. Records intent only —
 *  the operator-run reap tool performs the actual deletion. */
export const setMarked = (token: string | undefined, arns: string[], marked: boolean) =>
  post<{ ok: boolean; count: number }>("/api/inventory/mark", token, { arns, marked });

/** Force the API to recompute (bypass the 1h server cache), then callers reload. */
export async function bustCache(token: string | undefined) {
  const headers = authHeaders(token);
  await Promise.all([
    fetch("/api/cost?refresh=1", { headers }),
    fetch("/api/inventory?refresh=1", { headers }),
  ]);
}
