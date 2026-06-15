import type { ButtonHTMLAttributes, ReactNode } from "react";
import { cn } from "../lib/utils";

export function Card({ className, children }: { className?: string; children: ReactNode }) {
  return (
    <div className={cn("rounded-xl border border-neutral-800 bg-neutral-900/40", className)}>
      {children}
    </div>
  );
}

export function CardHeader({ title, right }: { title: string; right?: ReactNode }) {
  return (
    <div className="flex items-center justify-between border-b border-neutral-800 px-4 py-3">
      <h3 className="text-sm font-medium text-neutral-300">{title}</h3>
      {right}
    </div>
  );
}

export function CardBody({ className, children }: { className?: string; children: ReactNode }) {
  return <div className={cn("p-4", className)}>{children}</div>;
}

export function Stat({ label, value, sub }: { label: string; value: ReactNode; sub?: ReactNode }) {
  return (
    <Card>
      <CardBody>
        <div className="text-xs uppercase tracking-wide text-neutral-500">{label}</div>
        <div className="mt-1 text-2xl font-semibold tabular-nums text-neutral-100">{value}</div>
        {sub && <div className="mt-1 text-xs text-neutral-400">{sub}</div>}
      </CardBody>
    </Card>
  );
}

type Tone = "default" | "warn" | "danger" | "ok";
export function Badge({ children, tone = "default" }: { children: ReactNode; tone?: Tone }) {
  const tones: Record<Tone, string> = {
    default: "bg-neutral-800 text-neutral-300",
    ok: "bg-emerald-950 text-emerald-300 ring-1 ring-emerald-900",
    warn: "bg-amber-950 text-amber-300 ring-1 ring-amber-900",
    danger: "bg-red-950 text-red-300 ring-1 ring-red-900",
  };
  return (
    <span className={cn("inline-flex items-center rounded-full px-2 py-0.5 text-xs font-medium", tones[tone])}>
      {children}
    </span>
  );
}

export function Button({ className, ...props }: ButtonHTMLAttributes<HTMLButtonElement>) {
  return (
    <button
      className={cn(
        "inline-flex items-center gap-2 rounded-md bg-neutral-100 px-3 py-1.5 text-sm font-medium text-neutral-900 transition hover:bg-white disabled:opacity-50",
        className,
      )}
      {...props}
    />
  );
}

export function Spinner({ label }: { label?: string }) {
  return (
    <div className="flex items-center gap-3 text-sm text-neutral-400">
      <div className="h-4 w-4 animate-spin rounded-full border-2 border-neutral-700 border-t-neutral-300" />
      {label}
    </div>
  );
}
