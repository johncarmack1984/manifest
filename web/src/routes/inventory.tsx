import { useState } from "react";
import { ChevronRight } from "lucide-react";
import { useAuth } from "react-oidc-context";
import { getInventory, type ResourceRow } from "../api";
import { useAsync } from "../lib/useAsync";
import { Stat, Spinner } from "../components/ui";
import { cn } from "../lib/utils";

// Group-header tint when a group isn't a normal app (draws the eye to cruft).
const TONE: Record<string, string> = {
  orphan: "text-red-300",
  unclaimed: "text-amber-300",
  tooling: "text-neutral-500",
  "aws-managed": "text-neutral-500",
};

export default function Inventory() {
  const token = useAuth().user?.id_token;
  const { data, loading, error } = useAsync(() => getInventory(token), [token]);
  const [q, setQ] = useState("");
  const [region, setRegion] = useState("all");
  const [hideNoise, setHideNoise] = useState(true);
  const [open, setOpen] = useState<Set<string>>(new Set());

  if (loading) return <Spinner label="Loading inventory…" />;
  if (error || !data) return <div className="text-sm text-red-400">Error: {error}</div>;

  const isNoise = (c: string) => c === "aws-managed" || c === "tooling";
  const filtered = data.resources.filter(
    (r) =>
      (!hideNoise || !isNoise(r.category)) &&
      (region === "all" || r.region === region) &&
      (q === "" || `${r.arn} ${r.type} ${r.name}`.toLowerCase().includes(q.toLowerCase())),
  );

  // Group by app; resources with no app fall back to their category bucket.
  const groups = new Map<string, ResourceRow[]>();
  for (const r of filtered) {
    const key = r.app ?? r.category;
    const g = groups.get(key);
    if (g) g.push(r);
    else groups.set(key, [r]);
  }
  // Orphans first, then unclaimed, then apps by size.
  const rank = (items: ResourceRow[]) =>
    items[0].category === "orphan" ? 0 : items[0].category === "unclaimed" ? 1 : 2;
  const sorted = [...groups.entries()].sort(
    (a, b) => rank(a[1]) - rank(b[1]) || b[1].length - a[1].length,
  );

  const allOpen = sorted.length > 0 && open.size === sorted.length;
  const toggleAll = () => setOpen(allOpen ? new Set() : new Set(sorted.map(([k]) => k)));
  const toggle = (k: string) =>
    setOpen((s) => {
      const n = new Set(s);
      if (n.has(k)) n.delete(k);
      else n.add(k);
      return n;
    });

  return (
    <div className="space-y-4">
      <div className="grid grid-cols-4 gap-4">
        <Stat label="Resources" value={data.count} />
        <Stat label="Orphans" value={data.flags.orphans} sub="dead / handed-off" />
        <Stat label="Unclaimed" value={data.flags.unclaimed} sub="needs attribution" />
        <Stat label="Apps" value={Object.keys(data.byApp).length} />
      </div>

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
        <button
          onClick={toggleAll}
          className="rounded-md border border-neutral-800 px-3 py-1.5 text-sm text-neutral-400 hover:text-neutral-200"
        >
          {allOpen ? "Collapse all" : "Expand all"}
        </button>
      </div>

      <div className="space-y-2">
        {sorted.map(([key, items]) => {
          const isOpen = open.has(key);
          const cat = items[0].category;
          return (
            <div key={key} className="overflow-hidden rounded-lg border border-neutral-800 bg-neutral-900/30">
              <button
                onClick={() => toggle(key)}
                className="flex w-full items-center gap-2 px-4 py-2.5 text-left hover:bg-neutral-900/50"
              >
                <ChevronRight
                  className={cn("h-4 w-4 shrink-0 text-neutral-500 transition-transform", isOpen && "rotate-90")}
                />
                <span className={cn("font-medium", TONE[cat] ?? "text-neutral-200")}>{key}</span>
                <span className="text-sm tabular-nums text-neutral-500">×{items.length}</span>
              </button>
              {isOpen && (
                <div className="overflow-x-auto border-t border-neutral-800/60">
                  <table className="w-full table-fixed text-sm">
                    <colgroup>
                      <col style={{ width: "42%" }} />
                      <col style={{ width: "14%" }} />
                      <col style={{ width: "44%" }} />
                    </colgroup>
                    <tbody>
                      {items.map((r) => (
                        <tr key={r.arn} className="border-b border-neutral-800/40 last:border-0">
                          <td className="truncate px-4 py-1.5 text-neutral-300" title={r.arn}>
                            {r.name}
                          </td>
                          <td className="truncate px-4 py-1.5 text-neutral-500">{r.region}</td>
                          <td className="truncate px-4 py-1.5 text-neutral-500" title={r.reason}>
                            {r.type}
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}
