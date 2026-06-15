import { useAuth } from "react-oidc-context";
import { getCost, getInventory } from "../api";
import { useAsync } from "../lib/useAsync";
import { Card, CardHeader, CardBody, Stat, Spinner, Badge } from "../components/ui";
import { MonthlyBars, DailyLine } from "../charts";
import { usd } from "../lib/utils";

export default function Overview() {
  const token = useAuth().user?.id_token;
  const cost = useAsync(() => getCost(token), [token]);
  const inv = useAsync(() => getInventory(token), [token]);

  if (cost.loading || inv.loading) return <Spinner label="Loading dashboard…" />;
  if (cost.error || !cost.data) return <div className="text-sm text-red-400">Error: {cost.error}</div>;

  const c = cost.data;
  const months = c.byService;
  const current = months.at(-1)?.total ?? 0;
  const prev = months.at(-2)?.total ?? 0;
  const delta = prev > 0 ? ((current - prev) / prev) * 100 : 0;
  const uncovered = c.flags.uncoveredRegionsWithSpend;
  const oneOffs = c.runRate?.oneOffItems ?? [];

  return (
    <div className="space-y-6">
      <div className="grid grid-cols-2 gap-4 md:grid-cols-4">
        <Stat
          label="This month"
          value={usd(current)}
          sub={prev > 0 ? `${delta >= 0 ? "+" : ""}${delta.toFixed(0)}% vs prev` : undefined}
        />
        <Stat
          label="Run-rate / mo"
          value={usd(c.runRate?.runRateMonthly ?? 0)}
          sub={
            c.forecastNextMonth != null
              ? `spike-robust · CE trend ${usd(c.forecastNextMonth)}`
              : "spike-robust median"
          }
        />
        <Stat
          label="Resources"
          value={inv.data?.count ?? "—"}
          sub={`${inv.data?.indexedRegions.length ?? 0} indexed regions`}
        />
        <Stat label="Orphans" value={inv.data?.flags.orphans ?? "—"} sub="dead / handed-off" />
      </div>

      {uncovered.length > 0 && (
        <Card>
          <CardBody>
            <div className="flex flex-wrap items-center gap-2">
              <Badge tone="danger">blind spot</Badge>
              <span className="text-sm text-neutral-300">
                Spend in regions with no inventory coverage:{" "}
                <span className="font-medium text-neutral-100">{uncovered.join(", ")}</span>
              </span>
            </div>
          </CardBody>
        </Card>
      )}

      {oneOffs.length > 0 && (
        <Card>
          <CardHeader
            title={`One-off costs this month · ${usd(c.runRate.oneOffMtd)}`}
            right={<Badge>excluded from run-rate</Badge>}
          />
          <CardBody className="p-0">
            <table className="w-full text-sm">
              <tbody>
                {oneOffs.map((o) => (
                  <tr key={o.usageType} className="border-b border-neutral-800/60 last:border-0">
                    <td className="px-4 py-2 text-neutral-300">{o.usageType}</td>
                    <td className="px-4 py-2 text-right tabular-nums text-neutral-100">
                      {usd(o.amount)}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </CardBody>
        </Card>
      )}

      <Card>
        <CardHeader title="Monthly spend" />
        <CardBody>
          <MonthlyBars data={months} />
        </CardBody>
      </Card>

      <Card>
        <CardHeader title="Daily spend · last 30 days" />
        <CardBody>
          <DailyLine data={c.daily} />
        </CardBody>
      </Card>
    </div>
  );
}
