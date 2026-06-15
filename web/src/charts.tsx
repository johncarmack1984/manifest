import {
  ResponsiveContainer,
  BarChart,
  Bar,
  XAxis,
  YAxis,
  Tooltip,
  CartesianGrid,
  LineChart,
  Line,
} from "recharts";
import type { CostPeriod, DailyPoint } from "./api";
import { usd } from "./lib/utils";

const axis = { stroke: "#525252", fontSize: 11 } as const;
const grid = "#262626";
const tip = {
  background: "#171717",
  border: "1px solid #404040",
  borderRadius: 8,
  fontSize: 12,
} as const;

export function MonthlyBars({ data }: { data: CostPeriod[] }) {
  const rows = data.map((p) => ({ period: p.period.slice(0, 7), total: p.total }));
  return (
    <ResponsiveContainer width="100%" height={220}>
      <BarChart data={rows}>
        <CartesianGrid stroke={grid} vertical={false} />
        <XAxis dataKey="period" {...axis} />
        <YAxis {...axis} width={56} tickFormatter={(v: number) => `$${Math.round(v)}`} />
        <Tooltip contentStyle={tip} formatter={(v: number) => usd(v)} cursor={{ fill: "#ffffff10" }} />
        <Bar dataKey="total" fill="#22d3ee" radius={[3, 3, 0, 0]} />
      </BarChart>
    </ResponsiveContainer>
  );
}

export function DailyLine({ data }: { data: DailyPoint[] }) {
  const rows = data.map((d) => ({ date: d.date.slice(5), amount: d.amount }));
  return (
    <ResponsiveContainer width="100%" height={200}>
      <LineChart data={rows}>
        <CartesianGrid stroke={grid} vertical={false} />
        <XAxis dataKey="date" {...axis} minTickGap={24} />
        <YAxis {...axis} width={56} tickFormatter={(v: number) => `$${Math.round(v)}`} />
        <Tooltip contentStyle={tip} formatter={(v: number) => usd(v)} />
        <Line type="monotone" dataKey="amount" stroke="#a3e635" dot={false} strokeWidth={2} />
      </LineChart>
    </ResponsiveContainer>
  );
}
