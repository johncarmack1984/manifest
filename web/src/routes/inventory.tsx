import { useEffect, useState } from "react";
import { Check, ChevronRight, Columns3, ExternalLink, Pencil } from "lucide-react";
import { Menu } from "@base-ui-components/react/menu";
import { useAuth } from "react-oidc-context";
import {
  flexRender,
  getCoreRowModel,
  useReactTable,
  type ColumnDef,
  type OnChangeFn,
  type RowData,
  type TableMeta,
  type VisibilityState,
} from "@tanstack/react-table";
import { addApp, getInventory, reclassify, setMarked, updateApp, type InventoryData, type ResourceRow } from "../api";
import { Stat, Spinner, Button } from "../components/ui";
import { cn, usd } from "../lib/utils";
import { consoleUrl } from "../lib/console";

// Group-header tint when a group isn't a normal app (draws the eye to cruft).
const TONE: Record<string, string> = {
  orphan: "text-red-300",
  unclaimed: "text-amber-300",
  tooling: "text-neutral-500",
  "aws-managed": "text-neutral-500",
};

// Selection is shared across every group's table, so it rides table meta rather
// than TanStack's per-table row-selection state.
declare module "@tanstack/react-table" {
  interface TableMeta<TData extends RowData> {
    selected: Set<string>;
    toggleSelect: (arn: string) => void;
    toggleGroup: (items: ResourceRow[], on: boolean) => void;
    multiAccount: boolean;
  }
  interface ColumnMeta<TData extends RowData, TValue> {
    /** Human label in the Columns menu. */
    label?: string;
    width?: string;
    thClass?: string;
    tdClass?: string;
  }
}

const fmtCost = (c: number) => (c > 0 && c < 0.005 ? "<$0.01" : usd(c));

const columns: ColumnDef<ResourceRow>[] = [
  {
    id: "select",
    enableHiding: false,
    meta: { width: "2.5rem", thClass: "px-3 py-1.5", tdClass: "px-3 py-1.5" },
    header: ({ table }) => {
      const m = table.options.meta!;
      const rows = table.getRowModel().rows.map((r) => r.original);
      const allSel = rows.length > 0 && rows.every((r) => m.selected.has(r.arn));
      return (
        <input
          type="checkbox"
          aria-label="select all in group"
          checked={allSel}
          onChange={(e) => m.toggleGroup(rows, e.target.checked)}
          className="accent-neutral-300"
        />
      );
    },
    cell: ({ row, table }) => {
      const m = table.options.meta!;
      return (
        <input
          type="checkbox"
          aria-label={`select ${row.original.name}`}
          checked={m.selected.has(row.original.arn)}
          onChange={() => m.toggleSelect(row.original.arn)}
          className="accent-neutral-300"
        />
      );
    },
  },
  {
    id: "name",
    enableHiding: false,
    meta: { label: "name", width: "40%", tdClass: "truncate px-2 py-1.5 text-neutral-300" },
    header: "name",
    cell: ({ row }) => {
      const r = row.original;
      const url = consoleUrl(r);
      return (
        <span title={r.arn}>
          {url ? (
            <a
              href={url}
              target="_blank"
              rel="noreferrer"
              className="hover:text-white hover:underline"
              title="open in AWS console"
            >
              {r.name}
              <ExternalLink className="mb-0.5 ml-1 inline h-3 w-3 text-neutral-600" />
            </a>
          ) : (
            r.name
          )}
          {r.override && (
            <span className="ml-1.5 text-[10px] uppercase tracking-wide text-sky-400" title="manually classified">
              override
            </span>
          )}
          {r.mark && (
            <span className="ml-1.5 text-[10px] uppercase tracking-wide text-red-400" title="marked for deletion">
              marked
            </span>
          )}
        </span>
      );
    },
  },
  {
    id: "region",
    meta: { label: "region", width: "12%", tdClass: "truncate px-2 py-1.5 text-neutral-500" },
    header: "region",
    cell: ({ row, table }) => (
      <>
        {row.original.region}
        {table.options.meta!.multiAccount && row.original.accountName && (
          <span className="ml-1.5 text-neutral-600">· {row.original.accountName}</span>
        )}
      </>
    ),
  },
  {
    id: "account",
    meta: { label: "account", width: "12%", tdClass: "truncate px-2 py-1.5 text-neutral-500" },
    header: "account",
    cell: ({ row }) => row.original.accountName || row.original.account || "",
  },
  {
    id: "service",
    meta: { label: "service", width: "10%", tdClass: "truncate px-2 py-1.5 text-neutral-500" },
    header: "service",
    cell: ({ row }) => row.original.service,
  },
  {
    id: "type",
    meta: { label: "type", width: "24%", tdClass: "truncate px-2 py-1.5 text-neutral-500" },
    header: "type",
    cell: ({ row }) => <span title={row.original.reason}>{row.original.type}</span>,
  },
  {
    id: "stack",
    meta: { label: "stack", width: "16%", tdClass: "truncate px-2 py-1.5 text-neutral-500" },
    header: "stack",
    cell: ({ row }) => row.original.stack ?? "",
  },
  {
    id: "reason",
    meta: { label: "reason", width: "22%", tdClass: "truncate px-2 py-1.5 text-neutral-500" },
    header: "reason",
    cell: ({ row }) => row.original.reason,
  },
  {
    id: "lastReported",
    meta: { label: "last seen", width: "7rem", tdClass: "px-2 py-1.5 tabular-nums text-neutral-500" },
    header: "last seen",
    cell: ({ row }) => row.original.lastReported?.slice(0, 10) ?? "",
  },
  {
    id: "cost",
    meta: {
      label: "est $/mo",
      width: "6.5rem",
      thClass: "px-3 py-1.5 text-right font-medium",
      tdClass: "px-3 py-1.5 text-right tabular-nums text-neutral-400",
    },
    header: () => (
      <span title="estimated monthly run rate — the last ~2 weeks of resource-level Cost Explorer data, scaled to 30 days">
        est $/mo
      </span>
    ),
    cell: ({ row }) => (row.original.cost != null ? fmtCost(row.original.cost) : ""),
  },
];

// Hidden-by-default columns; the user's picks persist in localStorage.
const DEFAULT_VISIBILITY: VisibilityState = {
  account: false,
  service: false,
  stack: false,
  reason: false,
  lastReported: false,
};
const VIS_KEY = "manifest.inventory.columns";

function GroupTable({
  items,
  columnVisibility,
  onColumnVisibilityChange,
  meta,
}: {
  items: ResourceRow[];
  columnVisibility: VisibilityState;
  onColumnVisibilityChange: OnChangeFn<VisibilityState>;
  meta: TableMeta<ResourceRow>;
}) {
  const table = useReactTable({
    data: items,
    columns,
    state: { columnVisibility },
    onColumnVisibilityChange,
    getCoreRowModel: getCoreRowModel(),
    meta,
  });
  return (
    <table className="w-full table-fixed text-sm">
      <colgroup>
        {table.getVisibleLeafColumns().map((c) => (
          <col key={c.id} style={{ width: c.columnDef.meta?.width }} />
        ))}
      </colgroup>
      <thead>
        {table.getHeaderGroups().map((hg) => (
          <tr key={hg.id} className="border-b border-neutral-800/60 text-left text-[11px] uppercase tracking-wide text-neutral-600">
            {hg.headers.map((h) => (
              <th key={h.id} className={h.column.columnDef.meta?.thClass ?? "px-2 py-1.5 font-medium"}>
                {flexRender(h.column.columnDef.header, h.getContext())}
              </th>
            ))}
          </tr>
        ))}
      </thead>
      <tbody>
        {table.getRowModel().rows.map((row) => (
          <tr
            key={row.original.arn}
            className={cn(
              "border-b border-neutral-800/40 last:border-0 hover:bg-neutral-900/40",
              row.original.mark && "bg-red-950/20",
            )}
          >
            {row.getVisibleCells().map((cell) => (
              <td key={cell.id} className={cell.column.columnDef.meta?.tdClass}>
                {flexRender(cell.column.columnDef.cell, cell.getContext())}
              </td>
            ))}
          </tr>
        ))}
      </tbody>
    </table>
  );
}

export default function Inventory() {
  const token = useAuth().user?.id_token;
  const [data, setData] = useState<InventoryData | null>(null);
  const [error, setError] = useState("");
  const [refreshing, setRefreshing] = useState(false);

  const [q, setQ] = useState("");
  const [region, setRegion] = useState("all");
  const [account, setAccount] = useState("all");
  const [hideNoise, setHideNoise] = useState(true);
  const [onlyMarked, setOnlyMarked] = useState(false);
  const [open, setOpen] = useState<Set<string>>(new Set());

  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [target, setTarget] = useState("");
  const [actionError, setActionError] = useState("");

  // Add/edit-app form. `editKey` is the app being edited (null ⇒ adding a new one);
  // the same form serves both, so rules stay editable after creation.
  const emptyForm = { repo: "", patterns: "", types: "", protected: false, dead: false, reason: "" };
  const [showForm, setShowForm] = useState(false);
  const [editKey, setEditKey] = useState<string | null>(null);
  const [form, setForm] = useState(emptyForm);
  const [formError, setFormError] = useState("");

  // Column picks survive reloads; unknown/missing keys fall back to the defaults.
  const [columnVisibility, setColumnVisibility] = useState<VisibilityState>(() => {
    try {
      return {
        ...DEFAULT_VISIBILITY,
        ...(JSON.parse(localStorage.getItem(VIS_KEY) ?? "{}") as VisibilityState),
      };
    } catch {
      return DEFAULT_VISIBILITY;
    }
  });
  useEffect(() => {
    localStorage.setItem(VIS_KEY, JSON.stringify(columnVisibility));
  }, [columnVisibility]);

  // Initial load, then a background revalidate on each token renewal — the page
  // keeps showing the last data instead of blanking to the spinner. Mutations
  // update local state directly, so this only runs once per token.
  useEffect(() => {
    let live = true;
    setError("");
    getInventory(token)
      .then((d) => live && setData(d))
      .catch((e) => live && setError(String(e instanceof Error ? e.message : e)));
    return () => {
      live = false;
    };
  }, [token]);

  // Reconcile with the server (forced recompute) without blanking the page — used
  // only when a mutation's result can't be derived locally, or to recover from a failure.
  const refresh = async () => {
    setRefreshing(true);
    try {
      setData(await getInventory(token, true));
    } catch (e) {
      setActionError(String(e instanceof Error ? e.message : e));
    } finally {
      setRefreshing(false);
    }
  };

  if (error && !data) return <div className="text-sm text-red-400">Error: {error}</div>;
  if (!data) return <Spinner label="Loading inventory…" />;
  const inv = data;

  const isNoise = (c: string) => c === "aws-managed" || c === "tooling";
  const accounts = inv.byAccount ? Object.keys(inv.byAccount).sort() : [];
  const multiAccount = accounts.length > 1;
  // Counts + app list derived from resources so they react to optimistic edits.
  const orphans = inv.resources.filter((r) => r.category === "orphan").length;
  const unclaimed = inv.resources.filter((r) => r.category === "unclaimed").length;
  const markedCount = inv.resources.filter((r) => r.mark).length;
  // Picker shows every defined app (registry) plus any in use on a resource.
  const appNames = [
    ...new Set([...(inv.apps ?? []), ...inv.resources.map((r) => r.app)].filter(Boolean) as string[]),
  ].sort();

  const filtered = inv.resources.filter(
    (r) =>
      (!hideNoise || !isNoise(r.category)) &&
      (!onlyMarked || !!r.mark) &&
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

  // Apply a mutation: update the selected rows in place (instant), fire the request,
  // and only hit the server for a fresh view when we couldn't compute the result
  // locally (reconcile) or the request failed.
  const patch = (arns: Set<string>, fn: (r: ResourceRow) => ResourceRow) =>
    setData((d) =>
      d ? { ...d, resources: d.resources.map((r) => (arns.has(r.arn) ? fn(r) : r)) } : d,
    );
  const run = async (
    call: () => Promise<unknown>,
    optimistic: (r: ResourceRow) => ResourceRow,
    reconcile = false,
  ) => {
    if (selected.size === 0) return;
    const arns = new Set(selected);
    patch(arns, optimistic);
    setSelected(new Set());
    setTarget("");
    setActionError("");
    try {
      await call();
      if (reconcile) await refresh();
    } catch (e) {
      setActionError(String(e instanceof Error ? e.message : e));
      await refresh();
    }
  };
  const apply = (app: string | null) =>
    run(
      () => reclassify(token, [...selected], app),
      // Move: we know the result. Clear: the server recomputes the inferred class,
      // so leave the row as-is and reconcile in the background.
      app ? (r) => ({ ...r, app, category: "app", override: true }) : (r) => r,
      !app,
    );
  const markSelected = (marked: boolean) =>
    run(
      () => setMarked(token, [...selected], marked),
      (r) => ({ ...r, mark: marked ? "marked" : null }),
    );

  const openAdd = () => {
    setForm(emptyForm);
    setFormError("");
    setEditKey(null);
    setShowForm((v) => !v || editKey !== null);
  };
  const openEdit = (key: string) => {
    const m = inv.appMeta?.[key];
    if (!m) return;
    setForm({
      repo: key,
      patterns: (m.patterns ?? []).join(", "),
      types: (m.types ?? []).join(", "),
      protected: !!m.protected,
      dead: !!m.dead,
      reason: m.reason ?? "",
    });
    setFormError("");
    setEditKey(key);
    setShowForm(true);
  };
  const closeForm = () => {
    setShowForm(false);
    setEditKey(null);
    setForm(emptyForm);
  };
  const submitForm = async () => {
    const repo = form.repo.trim();
    if (!repo) return;
    const csv = (s: string) => s.split(",").map((x) => x.trim()).filter(Boolean);
    const body = {
      repo,
      patterns: csv(form.patterns),
      types: csv(form.types),
      protected: form.protected,
      dead: form.dead,
      reason: form.reason.trim(),
    };
    setFormError("");
    try {
      await (editKey ? updateApp(token, body) : addApp(token, body));
      closeForm();
      // Rules changed, so inferred classifications must recompute server-side.
      await refresh();
    } catch (e) {
      setFormError(String(e instanceof Error ? e.message : e));
    }
  };

  // Shared add/edit form body — rendered at the top for a new app, inline under an
  // app's group header when editing its rules. Plain JSX (not a component) so inputs
  // keep focus across re-renders.
  const field =
    "rounded-md border border-neutral-700 bg-neutral-900/60 px-3 py-1.5 text-sm text-neutral-200 outline-none placeholder:text-neutral-600 focus:border-neutral-500";
  const appForm = (
    <div className="flex flex-wrap items-end gap-3">
      <label className="flex flex-col gap-1 text-xs text-neutral-500">
        app name
        <input
          autoFocus={editKey === null}
          disabled={editKey !== null}
          value={form.repo}
          onChange={(e) => setForm({ ...form, repo: e.target.value })}
          placeholder="my-new-app"
          className={cn(field, "w-44", editKey !== null && "opacity-60")}
        />
      </label>
      <label className="flex flex-col gap-1 text-xs text-neutral-500">
        match patterns (comma-separated, case-insensitive substrings)
        <input
          autoFocus={editKey !== null}
          value={form.patterns}
          onChange={(e) => setForm({ ...form, patterns: e.target.value })}
          placeholder="my-new-app, mynewappstack"
          className={cn(field, "w-72")}
        />
      </label>
      <label className="flex flex-col gap-1 text-xs text-neutral-500">
        claim types (comma-separated, exact)
        <input
          value={form.types}
          onChange={(e) => setForm({ ...form, types: e.target.value })}
          placeholder="cloudwatch:alarm, sns:topic"
          className={cn(field, "w-56")}
        />
      </label>
      <label className="flex cursor-pointer select-none items-center gap-2 pb-1.5 text-sm text-neutral-400">
        <input
          type="checkbox"
          checked={form.protected}
          onChange={(e) => setForm({ ...form, protected: e.target.checked })}
          className="accent-neutral-300"
        />
        protected
      </label>
      <label className="flex cursor-pointer select-none items-center gap-2 pb-1.5 text-sm text-neutral-400">
        <input
          type="checkbox"
          checked={form.dead}
          onChange={(e) => setForm({ ...form, dead: e.target.checked })}
          className="accent-red-400"
        />
        dead
      </label>
      {form.dead && (
        <label className="flex flex-col gap-1 text-xs text-neutral-500">
          reason (shown on its orphans)
          <input
            value={form.reason}
            onChange={(e) => setForm({ ...form, reason: e.target.value })}
            placeholder="decommissioned; safe to delete"
            className={cn(field, "w-64")}
          />
        </label>
      )}
      <Button disabled={!form.repo.trim()} onClick={submitForm} className="mb-0.5">
        {editKey ? "Save" : "Add"}
      </Button>
      <button onClick={closeForm} className="pb-1.5 text-sm text-neutral-500 hover:text-neutral-300">
        Cancel
      </button>
      {formError && <span className="pb-1.5 text-sm text-red-400">{formError}</span>}
    </div>
  );

  return (
    <div className="space-y-4">
      <div className="grid grid-cols-2 gap-4 sm:grid-cols-3 lg:grid-cols-5">
        <Stat label="Resources" value={inv.count} />
        <Stat label="Orphans" value={orphans} sub="dead / handed-off" />
        <Stat label="Unclaimed" value={unclaimed} sub="needs attribution" />
        <Stat label="Marked" value={markedCount} sub="queued for reap" />
        <Stat label="Apps" value={appNames.length} />
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
          {inv.indexedRegions.map((r) => (
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
        <label className="flex cursor-pointer select-none items-center gap-2 text-sm text-neutral-400">
          <input
            type="checkbox"
            checked={onlyMarked}
            onChange={(e) => setOnlyMarked(e.target.checked)}
            className="accent-red-400"
          />
          Only marked
        </label>
        <button
          onClick={toggleAll}
          className="rounded-md border border-neutral-800 px-3 py-1.5 text-sm text-neutral-400 hover:text-neutral-200"
        >
          {allOpen ? "Collapse all" : "Expand all"}
        </button>
        <Menu.Root>
          <Menu.Trigger className="inline-flex items-center gap-1.5 rounded-md border border-neutral-800 px-3 py-1.5 text-sm text-neutral-400 hover:text-neutral-200">
            <Columns3 className="h-3.5 w-3.5" />
            Columns
          </Menu.Trigger>
          <Menu.Portal>
            <Menu.Positioner sideOffset={6} align="start">
              <Menu.Popup className="z-20 min-w-40 rounded-md border border-neutral-700 bg-neutral-900 py-1 text-sm shadow-lg outline-none">
                {columns
                  .filter((c) => c.enableHiding !== false)
                  .map((c) => {
                    const id = c.id ?? "";
                    return (
                      <Menu.CheckboxItem
                        key={id}
                        checked={columnVisibility[id] !== false}
                        onCheckedChange={(checked) =>
                          setColumnVisibility((v) => ({ ...v, [id]: checked }))
                        }
                        closeOnClick={false}
                        className="flex cursor-default select-none items-center gap-2 px-3 py-1.5 text-neutral-300 outline-none data-[highlighted]:bg-neutral-800"
                      >
                        <span className="inline-flex h-3.5 w-3.5 items-center justify-center">
                          <Menu.CheckboxItemIndicator>
                            <Check className="h-3.5 w-3.5" />
                          </Menu.CheckboxItemIndicator>
                        </span>
                        {c.meta?.label ?? id}
                      </Menu.CheckboxItem>
                    );
                  })}
              </Menu.Popup>
            </Menu.Positioner>
          </Menu.Portal>
        </Menu.Root>
        <button
          onClick={openAdd}
          className="rounded-md border border-neutral-800 px-3 py-1.5 text-sm text-neutral-400 hover:text-neutral-200"
        >
          + Add app
        </button>
        {refreshing && <span className="text-xs text-neutral-500">updating…</span>}
      </div>

      {showForm && editKey === null && (
        <div className="rounded-lg border border-neutral-800 bg-neutral-900/40 px-4 py-3">{appForm}</div>
      )}

      {actionError && (
        <div className="rounded-lg border border-red-900/50 bg-red-950/20 px-4 py-2.5 text-sm text-red-300">
          {actionError}
        </div>
      )}

      {inv.flags.notIndexed && inv.flags.notIndexed.length > 0 && (
        <div className="rounded-lg border border-amber-900/50 bg-amber-950/20 px-4 py-2.5 text-sm text-amber-300/90">
          {inv.flags.notIndexed.length} member account
          {inv.flags.notIndexed.length > 1 ? "s" : ""} not inventoried —{" "}
          {inv.flags.notIndexed.map((n) => n.accountName || n.account).join(", ")}.{" "}
          <span className="text-amber-300/60">
            deploy the inventory role there (just member-deploy) or enable Resource Explorer.
          </span>
        </div>
      )}

      {selected.size > 0 && (
        <div className="sticky top-2 z-10 flex flex-wrap items-center gap-3 rounded-lg border border-neutral-700 bg-neutral-900 px-4 py-2.5 shadow-lg">
          <span className="text-sm text-neutral-300">{selected.size} selected — move to</span>
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
          <Button disabled={!target.trim()} onClick={() => apply(target.trim())}>
            Move
          </Button>
          <button
            onClick={() => apply(null)}
            className="text-sm text-neutral-400 hover:text-neutral-200"
            title="Remove any manual override (back to inferred classification)"
          >
            Clear override
          </button>
          <span className="h-4 w-px bg-neutral-700" />
          <button
            onClick={() => markSelected(true)}
            className="text-sm text-red-400 hover:text-red-300"
            title="Flag for deletion — the reap tool deletes marked resources (nothing is deleted here)"
          >
            Mark for deletion
          </button>
          <button
            onClick={() => markSelected(false)}
            className="text-sm text-neutral-400 hover:text-neutral-200"
          >
            Unmark
          </button>
          <button
            onClick={() => setSelected(new Set())}
            className="text-sm text-neutral-500 hover:text-neutral-300"
          >
            Deselect
          </button>
        </div>
      )}

      <div className="space-y-2">
        {sorted.map(([key, items]) => {
          const isOpen = open.has(key);
          const cat = items[0].category;
          return (
            <div key={key} className="overflow-hidden rounded-lg border border-neutral-800 bg-neutral-900/30">
              <div className="flex w-full items-center gap-2 px-4 py-2.5 hover:bg-neutral-900/50">
                <button
                  onClick={() => toggle(key)}
                  className="flex min-w-0 flex-1 items-center gap-2 text-left"
                >
                  <ChevronRight
                    className={cn("h-4 w-4 shrink-0 text-neutral-500 transition-transform", isOpen && "rotate-90")}
                  />
                  <span className={cn("font-medium", TONE[cat] ?? "text-neutral-200")}>{key}</span>
                  <span className="text-sm tabular-nums text-neutral-500">×{items.length}</span>
                </button>
                {inv.appMeta?.[key] && (
                  <button
                    onClick={() => openEdit(key)}
                    className="shrink-0 text-neutral-600 hover:text-neutral-200"
                    title="edit this app's match rules"
                  >
                    <Pencil className="h-3.5 w-3.5" />
                  </button>
                )}
                {inv.byAppCost?.[key] != null && (
                  <span
                    className="shrink-0 text-sm tabular-nums text-neutral-400"
                    title="current-month spend attributed via the CloudFormation stack-name tag"
                  >
                    {usd(inv.byAppCost[key])}/mo
                  </span>
                )}
              </div>
              {showForm && editKey === key && (
                <div className="border-t border-neutral-800/60 bg-neutral-900/40 px-4 py-3">{appForm}</div>
              )}
              {isOpen && (
                <div className="overflow-x-auto border-t border-neutral-800/60">
                  <GroupTable
                    items={items}
                    columnVisibility={columnVisibility}
                    onColumnVisibilityChange={setColumnVisibility}
                    meta={{ selected, toggleSelect, toggleGroup, multiAccount }}
                  />
                </div>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}
