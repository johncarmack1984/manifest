import { lazy, Suspense, useState } from "react";
import type { ReactNode } from "react";
import {
  createRootRoute,
  createRoute,
  createRouter,
  Outlet,
  Link,
  useRouterState,
} from "@tanstack/react-router";
import { RefreshCw } from "lucide-react";
import { useAuth } from "react-oidc-context";
import { useConfig, cognitoLogoutUrl } from "./config";
import { bustCache } from "./api";
import { Button, Spinner } from "./components/ui";
import { cn } from "./lib/utils";

// Lazy routes keep recharts + page code out of the initial entry bundle.
const Overview = lazy(() => import("./routes/overview"));
const Cost = lazy(() => import("./routes/cost"));
const Inventory = lazy(() => import("./routes/inventory"));

type NavPath = "/" | "/cost" | "/inventory";

function Center({ children }: { children: ReactNode }) {
  return <div className="flex min-h-screen items-center justify-center px-4">{children}</div>;
}

function Layout() {
  const auth = useAuth();
  const cfg = useConfig();
  const [busting, setBusting] = useState(false);

  if (auth.isLoading)
    return (
      <Center>
        <Spinner label="Loading…" />
      </Center>
    );
  if (auth.error)
    return (
      <Center>
        <div className="text-sm text-red-400">Auth error: {auth.error.message}</div>
      </Center>
    );
  if (!auth.isAuthenticated)
    return (
      <Center>
        <div className="text-center">
          <div className="mb-1 text-lg font-semibold">manifest</div>
          <div className="mb-5 text-sm text-neutral-500">account {cfg.accountId}</div>
          <Button onClick={() => void auth.signinRedirect()}>Sign in with Cognito</Button>
        </div>
      </Center>
    );

  return (
    <div className="min-h-screen">
      <header className="border-b border-neutral-800">
        <div className="mx-auto flex max-w-6xl items-center justify-between px-4 py-3">
          <div className="flex items-center gap-6">
            <span className="font-semibold">infra</span>
            <nav className="flex gap-1 text-sm">
              <NavLink to="/">Overview</NavLink>
              <NavLink to="/cost">Cost</NavLink>
              <NavLink to="/inventory">Inventory</NavLink>
            </nav>
          </div>
          <div className="flex items-center gap-3 text-sm text-neutral-400">
            <button
              disabled={busting}
              title="Recompute now (bypass the 1h cache)"
              onClick={async () => {
                setBusting(true);
                await bustCache(auth.user?.id_token).catch(() => {});
                window.location.reload();
              }}
              className="inline-flex items-center gap-1.5 text-neutral-300 hover:text-white disabled:opacity-50"
            >
              <RefreshCw size={14} className={busting ? "animate-spin" : ""} />
              {busting ? "Refreshing…" : "Refresh"}
            </button>
            <span className="hidden text-neutral-600 sm:inline">·</span>
            <span className="hidden sm:inline">{String(auth.user?.profile.email ?? "")}</span>
            <button
              className="text-neutral-300 hover:text-white"
              onClick={() => {
                void auth.removeUser();
                window.location.href = cognitoLogoutUrl(cfg);
              }}
            >
              Sign out
            </button>
          </div>
        </div>
      </header>
      <main className="mx-auto max-w-6xl px-4 py-6">
        <Suspense
          fallback={
            <div className="py-10">
              <Spinner label="Loading…" />
            </div>
          }
        >
          <Outlet />
        </Suspense>
      </main>
    </div>
  );
}

function NavLink({ to, children }: { to: NavPath; children: ReactNode }) {
  const path = useRouterState({ select: (s) => s.location.pathname });
  const active = path === to;
  return (
    <Link
      to={to}
      className={cn(
        "rounded-md px-3 py-1.5",
        active ? "bg-neutral-800 text-white" : "text-neutral-400 hover:text-neutral-200",
      )}
    >
      {children}
    </Link>
  );
}

const rootRoute = createRootRoute({ component: Layout });
const overviewRoute = createRoute({ getParentRoute: () => rootRoute, path: "/", component: Overview });
const costRoute = createRoute({ getParentRoute: () => rootRoute, path: "/cost", component: Cost });
const inventoryRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/inventory",
  component: Inventory,
});
const callbackRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/auth/callback",
  component: Overview,
});

const routeTree = rootRoute.addChildren([overviewRoute, costRoute, inventoryRoute, callbackRoute]);

export const router = createRouter({ routeTree });

declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router;
  }
}
