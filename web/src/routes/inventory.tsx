import { useState } from "react";
import { ChevronRight, ExternalLink } from "lucide-react";
import { useAuth } from "react-oidc-context";
import { getInventory, reclassify, type ResourceRow } from "../api";
import { useAsync } from "../lib/useAsync";
import { Stat, Spinner, Button } from "../components/ui";
import { cn } from "../lib/utils";
import { consoleUrl } from "../lib/console";

// Group-header tint when a group isn't a normal app (draws the eye to cruft).
const TONE: Record<string, string> = {
  orphan: "text-red-300",
  unclaimed: "text-amber-300",
  tooling: "text-neutral-500",
  "aws-managed": "text-neutral-500",
};

export default function Inventory() {
  const token = useAuth().user?.id_token;
  // nonce bumps force a server-side recompute (so a reclassify shows up immediately).
  const [nonce, setNonce] = useState(0);
  const { data, loading, error } = useAsync(() => getInventory(token, nonce > 0), [token, nonce]);
  const reload = () => setNonce((n) => n + 1);

  const [q, setQ] = useState("");
  const [region, setRegion] = useState("all");
  const [account, setAccount] = useState("all");
  const [hideNoise, setHideNoise] = useState(true);
  const [open, setOpen] = useState<Set<string>>(new Set());

  // Bulk reclassification.
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [target, setTarget] = useState("");
  const [busy, setBusy] = useState(false);
  const [actionError, setActionError] = useState("");

  if (loading) return <Spinner label="Loading inventory…" />;
  if (error || !data) return <div className="text-sm text-red-400">Error: {error}</div>;

  const isNoise = (c: string) => c === "aws-managed" || c === "tooling";
  const accounts = data.byAccount ? Object.keys(data.byAccount).sort() : [];
  const multiAccount = accounts.length > 1;
  const appNames = Object.keys(data.byApp).sort();
  const filtered = data.resources.filter(
    (r) =>
      (!hideNoise || !isNoise(r.category)) &&
      (region === "all" || r.region === region) &&
      (account === "all" || r.accountName === account || r.account === account) &&
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

  // ---- selection ----
  const toggleSelect = (arn: string) =>
    setSelected((s) => {
      const n = new Set(s);
      if (n.has(arn)) n.delete(arn);
      else n.add(arn);
      return n;
    });
  const toggleGroup = (items: ResourceRow[], on: boolean) =>
    setSelected((s) => {
      const n = new Set(s);
      for (const r of items) on ? n.add(r.arn) : n.delete(r.arn);
      return n;
    });

  const apply = async (app: string | null) => {
    if (selected.size === 0) return;
    setBusy(true);
    setActionError("");
    try {
      await reclassify(token, [...selected], app);
      setSelected(new Set());
      setTarget("");
      reload();
    } catch (e) {
      setActionError(String(e instanceof Error ? e.message : e));
    } finally {
      setBusy(false);
    }
  };

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
        {multiAccount && (
          <select
            value={account}
            onChange={(e) => setAccount(e.target.value)}
            className="rounded-md border border-neutral-800 bg-neutral-900/40 px-3 py-1.5 text-sm outline-none"
          >
            <option value="all">all accounts</option>
            {accounts.map((a) => (
              <option key={a} value={a}>
                {a}
              </option>
            ))}
          </select>
        )}
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

      {data.flags.notIndexed && data.flags.notIndexed.length > 0 && (
        <div className="rounded-lg border border-amber-900/50 bg-amber-950/20 px-4 py-2.5 text-sm text-amber-300/90">
          {data.flags.notIndexed.length} member account
          {data.flags.notIndexed.length > 1 ? "s" : ""} not inventoried —{" "}
          {data.flags.notIndexed.map((n) => n.accountName || n.account).join(", ")}.{" "}
          <span className="text-amber-300/60">
            deploy the inventory role there (just member-deploy) or enable Resource Explorer.
          </span>
        </div>
      )}

      {selected.size > 0 && (
        <div className="sticky top-2 z-10 flex flex-wrap items-center gap-3 rounded-lg border border-neutral-700 bg-neutral-900 px-4 py-2.5 shadow-lg">
          <span className="text-sm text-neutral-300">
            {selected.size} selected — move to
          </span>
          <input
            list="manifest-apps"
            value={target}
            onChange={(e) => setTarget(e.target.value)}
            placeholder="app…"
            className="w-48 rounded-md border border-neutral-700 bg-neutral-900/60 px-3 py-1.5 text-sm outline-none placeholder:text-neutral-600 focus:border-neutral-500"
          />
          <datalist id="manifest-apps">
            {appNames.map((a) => (
              <option key={a} value={a} />
            ))}
          </datalist>
          <Button disabled={busy || !target.trim()} onClick={() => apply(target.trim())}>
            {busy ? "Moving…" : "Move"}
          </Button>
          <button
            onClick={() => apply(null)}
            disabled={busy}
            className="text-sm text-neutral-400 hover:text-neutral-200 disabled:opacity-50"
            title="Remove any manual override (back to inferred classification)"
          >
            Clear override
          </button>
          <button
            onClick={() => setSelected(new Set())}
            className="text-sm text-neutral-500 hover:text-neutral-300"
          >
            Deselect
          </button>
          {actionError && <span className="text-sm text-red-400">{actionError}</span>}
        </div>
      )}

      <div className="space-y-2">
        {sorted.map(([key, items]) => {
          const isOpen = open.has(key);
          const cat = items[0].category;
          const allSel = items.every((r) => selected.has(r.arn));
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
                      <col style={{ width: "2.5rem" }} />
                      <col style={{ width: "40%" }} />
                      <col style={{ width: "14%" }} />
                      <col style={{ width: "34%" }} />
                      <col style={{ width: "2.75rem" }} />
                    </colgroup>
                    <thead>
                      <tr className="border-b border-neutral-800/60 text-left text-[11px] uppercase tracking-wide text-neutral-600">
                        <th className="px-3 py-1.5">
                          <input
                            type="checkbox"
                            aria-label="select all in group"
                            checked={allSel}
                            onChange={(e) => toggleGroup(items, e.target.checked)}
                            className="accent-neutral-300"
                          />
                        </th>
                        <th className="px-2 py-1.5 font-medium">name</th>
                        <th className="px-2 py-1.5 font-medium">region</th>
                        <th className="px-2 py-1.5 font-medium">type</th>
                        <th className="px-2 py-1.5"></th>
                      </tr>
                    </thead>
                    <tbody>
                      {items.map((r) => {
                        const url = consoleUrl(r);
                        return (
                          <tr key={r.arn} className="border-b border-neutral-800/40 last:border-0 hover:bg-neutral-900/40">
                            <td className="px-3 py-1.5">
                              <input
                                type="checkbox"
                                aria-label={`select ${r.name}`}
                                checked={selected.has(r.arn)}
                                onChange={() => toggleSelect(r.arn)}
                                className="accent-neutral-300"
                              />
                            </td>
                            <td className="truncate px-2 py-1.5 text-neutral-300" title={r.arn}>
                              {r.name}
                              {r.override && (
                                <span className="ml-1.5 text-[10px] uppercase tracking-wide text-sky-400" title="manually classified">
                                  override
                                </span>
                              )}
                            </td>
                            <td className="truncate px-2 py-1.5 text-neutral-500">
                              {r.region}
                              {multiAccount && r.accountName && (
                                <span className="ml-1.5 text-neutral-600">· {r.accountName}</span>
                              )}
                            </td>
                            <td className="truncate px-2 py-1.5 text-neutral-500" title={r.reason}>
                              {r.type}
                            </td>
                            <td className="px-2 py-1.5 text-right">
                              {url && (
                                <a
                                  href={url}
                                  target="_blank"
                                  rel="noreferrer"
                                  className="text-neutral-600 hover:text-neutral-200"
                                  title="open in AWS console"
                                >
                                  <ExternalLink className="inline h-3.5 w-3.5" />
                                </a>
                              )}
                            </td>
                          </tr>
                        );
                      })}
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
