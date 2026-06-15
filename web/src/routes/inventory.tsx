import { useState } from "react";
import { useAuth } from "react-oidc-context";
import { getInventory } from "../api";
import { useAsync } from "../lib/useAsync";
import { Card, CardHeader, CardBody, Stat, Spinner, Badge } from "../components/ui";
import { cn } from "../lib/utils";

export default function Inventory() {
  const token = useAuth().user?.id_token;
  const { data, loading, error } = useAsync(() => getInventory(token), [token]);
  const [q, setQ] = useState("");
  const [region, setRegion] = useState("all");
  const [hideManaged, setHideManaged] = useState(true);

  if (loading) return <Spinner label="Loading inventory…" />;
  if (error || !data) return <div className="text-sm text-red-400">Error: {error}</div>;

  const managedTypes = new Set(data.resources.filter((r) => r.managed).map((r) => r.type));
  const types = Object.entries(data.byType).sort((a, b) => b[1] - a[1]);

  const rows = data.resources.filter(
    (r) =>
      (!hideManaged || !r.managed) &&
      (region === "all" || r.region === region) &&
      (q === "" || `${r.arn} ${r.type} ${r.service}`.toLowerCase().includes(q.toLowerCase())),
  );

  return (
    <div className="space-y-4">
      <div className="grid grid-cols-3 gap-4">
        <Stat label="Your resources" value={data.ownedCount} sub={`of ${data.count} total`} />
        <Stat
          label="Untagged"
          value={data.flags.untaggedOwned}
          sub={`of ${data.flags.untaggedCount} incl. AWS-managed`}
        />
        <Stat label="Regions" value={Object.keys(data.byRegion).length} />
      </div>

      <Card>
        <CardHeader
          title="By type"
          right={<span className="text-xs text-neutral-500">{types.length} types · dim = AWS-managed</span>}
        />
        <CardBody>
          <div className="flex flex-wrap gap-2">
            {types.map(([t, n]) => (
              <span
                key={t}
                className={cn(
                  "rounded-md border px-2 py-1 text-xs",
                  managedTypes.has(t)
                    ? "border-neutral-800/70 text-neutral-600"
                    : "border-neutral-700 text-neutral-300",
                )}
              >
                {t} <span className="tabular-nums text-neutral-500">×{n}</span>
              </span>
            ))}
          </div>
        </CardBody>
      </Card>

      <div className="flex flex-wrap items-center gap-3">
        <input
          value={q}
          onChange={(e) => setQ(e.target.value)}
          placeholder="filter by arn / type / service…"
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
            checked={hideManaged}
            onChange={(e) => setHideManaged(e.target.checked)}
            className="accent-neutral-300"
          />
          Hide AWS-managed
        </label>
      </div>

      <Card>
        <CardBody className="p-0">
          <div className="overflow-x-auto">
            <table className="w-full table-fixed text-sm">
              <colgroup>
                <col style={{ width: "27%" }} />
                <col style={{ width: "11%" }} />
                <col style={{ width: "16%" }} />
                <col style={{ width: "34%" }} />
                <col style={{ width: "12%" }} />
              </colgroup>
              <thead>
                <tr className="border-b border-neutral-800 text-left text-xs uppercase tracking-wide text-neutral-500">
                  <th className="px-4 py-2 font-medium">Type</th>
                  <th className="px-4 py-2 font-medium">Region</th>
                  <th className="px-4 py-2 font-medium">Service</th>
                  <th className="px-4 py-2 font-medium">Name</th>
                  <th className="px-4 py-2 text-right font-medium">Tag</th>
                </tr>
              </thead>
              <tbody>
                {rows.slice(0, 500).map((r) => (
                  <tr key={r.arn} className="border-b border-neutral-800/50 last:border-0">
                    <td className="truncate px-4 py-2 text-neutral-300" title={r.type}>
                      {r.type}
                    </td>
                    <td className="truncate px-4 py-2 text-neutral-400">{r.region}</td>
                    <td className="truncate px-4 py-2 text-neutral-400">{r.service}</td>
                    <td className="truncate px-4 py-2 text-neutral-500" title={r.arn}>
                      {r.name}
                    </td>
                    <td className="px-4 py-2 text-right">
                      {r.managed ? (
                        <Badge>managed</Badge>
                      ) : r.untagged ? (
                        <Badge tone="warn">untagged</Badge>
                      ) : null}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          {rows.length > 500 && (
            <div className="px-4 py-2 text-xs text-neutral-500">
              showing first 500 of {rows.length}
            </div>
          )}
        </CardBody>
      </Card>
    </div>
  );
}
