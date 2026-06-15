import { useState } from "react";
import { useAuth } from "react-oidc-context";
import { getInventory } from "../api";
import { useAsync } from "../lib/useAsync";
import { Card, CardHeader, CardBody, Stat, Spinner } from "../components/ui";
import { cn } from "../lib/utils";

const CAT_CLASS: Record<string, string> = {
  orphan: "border-red-900 bg-red-950/40 text-red-300",
  unclaimed: "border-amber-900 bg-amber-950/30 text-amber-300",
  app: "border-emerald-900 bg-emerald-950/30 text-emerald-300",
  tooling: "border-neutral-800 text-neutral-600",
  "aws-managed": "border-neutral-800 text-neutral-600",
};

const RANK: Record<string, number> = { orphan: 0, unclaimed: 1, app: 2, tooling: 3, "aws-managed": 4 };

function CatPill({ c }: { c: string }) {
  return (
    <span className={cn("rounded-md border px-2 py-0.5 text-xs", CAT_CLASS[c] ?? "border-neutral-700 text-neutral-400")}>
      {c}
    </span>
  );
}

export default function Inventory() {
  const token = useAuth().user?.id_token;
  const { data, loading, error } = useAsync(() => getInventory(token), [token]);
  const [q, setQ] = useState("");
  const [region, setRegion] = useState("all");
  const [app, setApp] = useState("all");
  const [hideNoise, setHideNoise] = useState(true);

  if (loading) return <Spinner label="Loading inventory…" />;
  if (error || !data) return <div className="text-sm text-red-400">Error: {error}</div>;

  const isNoise = (c: string) => c === "aws-managed" || c === "tooling";
  const apps = Object.entries(data.byApp).sort((a, b) => b[1] - a[1]);

  const rows = data.resources
    .filter(
      (r) =>
        (!hideNoise || !isNoise(r.category)) &&
        (region === "all" || r.region === region) &&
        (app === "all" || r.app === app) &&
        (q === "" || `${r.arn} ${r.type} ${r.name}`.toLowerCase().includes(q.toLowerCase())),
    )
    .sort((a, b) => (RANK[a.category] ?? 5) - (RANK[b.category] ?? 5));

  return (
    <div className="space-y-4">
      <div className="grid grid-cols-4 gap-4">
        <Stat label="Resources" value={data.count} />
        <Stat label="Orphans" value={data.flags.orphans} sub="dead / handed-off" />
        <Stat label="Unclaimed" value={data.flags.unclaimed} sub="needs attribution" />
        <Stat label="Apps" value={apps.length} />
      </div>

      <Card>
        <CardHeader title="By app" right={<span className="text-xs text-neutral-500">click to filter</span>} />
        <CardBody>
          <div className="flex flex-wrap gap-2">
            {apps.map(([a, n]) => (
              <button
                key={a}
                onClick={() => setApp(app === a ? "all" : a)}
                className={cn(
                  "rounded-md border px-2 py-1 text-xs",
                  app === a
                    ? "border-neutral-500 text-white"
                    : "border-neutral-800 text-neutral-400 hover:text-neutral-200",
                )}
              >
                {a} <span className="tabular-nums text-neutral-500">×{n}</span>
              </button>
            ))}
          </div>
        </CardBody>
      </Card>

      <div className="flex flex-wrap items-center gap-3">
        <input
          value={q}
          onChange={(e) => setQ(e.target.value)}
          placeholder="filter by name / type / arn…"
          className="min-w-48 flex-1 rounded-md border border-neutral-800 bg-neutral-900/40 px-3 py-1.5 text-sm outline-none placeholder:text-neutral-600 focus:border-neutral-600"
        />
        <select
          value={region}
          onChange={(e) => setRegion(e.target.value)}
          className="rounded-md border border-neutral-800 bg-neutral-900/40 px-3 py-1.5 text-sm outline-none"
        >
          <option value="all">all regions</option>
          {data.indexedRegions.map((r) => (
            <option key={r} value={r}>
              {r}
            </option>
          ))}
        </select>
        <label className="flex cursor-pointer select-none items-center gap-2 text-sm text-neutral-400">
          <input
            type="checkbox"
            checked={hideNoise}
            onChange={(e) => setHideNoise(e.target.checked)}
            className="accent-neutral-300"
          />
          Hide AWS-managed + tooling
        </label>
      </div>

      <Card>
        <CardBody className="p-0">
          <div className="overflow-x-auto">
            <table className="w-full table-fixed text-sm">
              <colgroup>
                <col style={{ width: "12%" }} />
                <col style={{ width: "16%" }} />
                <col style={{ width: "10%" }} />
                <col style={{ width: "28%" }} />
                <col style={{ width: "34%" }} />
              </colgroup>
              <thead>
                <tr className="border-b border-neutral-800 text-left text-xs uppercase tracking-wide text-neutral-500">
                  <th className="px-4 py-2 font-medium">Category</th>
                  <th className="px-4 py-2 font-medium">App</th>
                  <th className="px-4 py-2 font-medium">Region</th>
                  <th className="px-4 py-2 font-medium">Name</th>
                  <th className="px-4 py-2 font-medium">Type / reason</th>
                </tr>
              </thead>
              <tbody>
                {rows.slice(0, 500).map((r) => (
                  <tr
                    key={r.arn}
                    className={cn(
                      "border-b border-neutral-800/50 last:border-0",
                      r.category === "orphan" && "bg-red-950/10",
                    )}
                  >
                    <td className="px-4 py-2">
                      <CatPill c={r.category} />
                    </td>
                    <td className="truncate px-4 py-2 text-neutral-300">{r.app ?? "—"}</td>
                    <td className="truncate px-4 py-2 text-neutral-500">{r.region}</td>
                    <td className="truncate px-4 py-2 text-neutral-300" title={r.arn}>
                      {r.name}
                    </td>
                    <td className="truncate px-4 py-2 text-neutral-500" title={r.reason}>
                      {r.type}
                      {r.category === "orphan" || r.category === "unclaimed" ? ` · ${r.reason}` : ""}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          {rows.length > 500 && (
            <div className="px-4 py-2 text-xs text-neutral-500">showing first 500 of {rows.length}</div>
          )}
        </CardBody>
      </Card>
    </div>
  );
}
