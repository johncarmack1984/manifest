import { Tabs } from "@base-ui-components/react/tabs";
import type { ReactNode } from "react";
import { useAuth } from "react-oidc-context";
import { getCost, type CostPeriod } from "../api";
import { useAsync } from "../lib/useAsync";
import { Card, CardHeader, CardBody, Spinner } from "../components/ui";
import { MonthlyBars } from "../charts";
import { usd } from "../lib/utils";

export default function Cost() {
  const token = useAuth().user?.id_token;
  const { data, loading, error } = useAsync(() => getCost(token), [token]);

  if (loading && !data) return <Spinner label="Loading cost…" />;
  if (error || !data) return <div className="text-sm text-red-400">Error: {error}</div>;

  return (
    <Tabs.Root defaultValue="account" className="space-y-4">
      <Tabs.List className="inline-flex gap-1 rounded-lg border border-neutral-800 bg-neutral-900/40 p-1">
        <Tab value="account">By account</Tab>
        <Tab value="service">By service</Tab>
        <Tab value="region">By region</Tab>
      </Tabs.List>
      <Tabs.Panel value="account">
        <Breakdown periods={data.byAccount} />
      </Tabs.Panel>
      <Tabs.Panel value="service">
        <Breakdown periods={data.byService} />
      </Tabs.Panel>
      <Tabs.Panel value="region">
        <Breakdown periods={data.byRegion} />
      </Tabs.Panel>
    </Tabs.Root>
  );
}

function Tab({ value, children }: { value: string; children: ReactNode }) {
  return (
    <Tabs.Tab
      value={value}
      className="cursor-pointer rounded-md px-3 py-1.5 text-sm text-neutral-400 outline-none data-[selected]:bg-neutral-800 data-[selected]:text-white"
    >
      {children}
    </Tabs.Tab>
  );
}

function Breakdown({ periods }: { periods: CostPeriod[] }) {
  const latest = periods.at(-1);
  const rows = [...(latest?.groups ?? [])]
    .filter((g) => g.amount > 0.005)
    .sort((a, b) => b.amount - a.amount);
  const total = rows.reduce((s, r) => s + r.amount, 0);

  return (
    <div className="space-y-4">
      <Card>
        <CardHeader title="Monthly" />
        <CardBody>
          <MonthlyBars data={periods} />
        </CardBody>
      </Card>
      <Card>
        <CardHeader title={`Breakdown · ${latest?.period?.slice(0, 7) ?? ""}`} />
        <CardBody className="p-0">
          <table className="w-full text-sm">
            <tbody>
              {rows.map((r) => (
                <tr key={r.key} className="border-b border-neutral-800/60 last:border-0">
                  <td className="px-4 py-2 text-neutral-300">{r.key}</td>
                  <td className="px-4 py-2 text-right tabular-nums text-neutral-100">{usd(r.amount)}</td>
                  <td className="w-16 px-4 py-2 text-right tabular-nums text-neutral-500">
                    {total > 0 ? `${((r.amount / total) * 100).toFixed(0)}%` : ""}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </CardBody>
      </Card>
    </div>
  );
}
