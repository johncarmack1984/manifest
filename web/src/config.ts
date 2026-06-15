import { createContext, useContext } from "react";

export interface AppConfig {
  accountId: string;
  appUrl: string;
  indexedRegions: string[];
  cognito: {
    region: string;
    userPoolId: string;
    clientId: string;
    hostedDomain: string;
  };
}

export async function loadConfig(): Promise<AppConfig> {
  const r = await fetch("/api/config");
  if (!r.ok) throw new Error(`/api/config returned ${r.status}`);
  return r.json();
}

export const ConfigContext = createContext<AppConfig | null>(null);

export function useConfig(): AppConfig {
  const c = useContext(ConfigContext);
  if (!c) throw new Error("ConfigContext not provided");
  return c;
}

/** Cognito logout isn't standard OIDC end-session; hit the hosted UI directly. */
export function cognitoLogoutUrl(cfg: AppConfig): string {
  const u = new URL(`https://${cfg.cognito.hostedDomain}/logout`);
  u.searchParams.set("client_id", cfg.cognito.clientId);
  u.searchParams.set("logout_uri", cfg.appUrl || window.location.origin);
  return u.toString();
}
