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
}
export interface InventoryData {
  count: number;
  resources: ResourceRow[];
  byRegion: Record<string, number>;
  byApp: Record<string, number>;
  byCategory: Record<string, number>;
  flags: { orphans: number; unclaimed: number };
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
export const getInventory = (token: string | undefined) =>
  get<InventoryData>("/api/inventory", token);

/** Force the API to recompute (bypass the 1h server cache), then callers reload. */
export async function bustCache(token: string | undefined) {
  const headers = authHeaders(token);
  await Promise.all([
    fetch("/api/cost?refresh=1", { headers }),
    fetch("/api/inventory?refresh=1", { headers }),
  ]);
}
