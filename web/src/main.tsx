import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { AuthProvider } from "react-oidc-context";
import { RouterProvider } from "@tanstack/react-router";
import { loadConfig, ConfigContext } from "./config";
import { router } from "./router";
import "./index.css";

const root = createRoot(document.getElementById("root")!);

loadConfig()
  .then((cfg) => {
    const { region, userPoolId, clientId, hostedDomain } = cfg.cognito;
    const authority = `https://cognito-idp.${region}.amazonaws.com/${userPoolId}`;

    const oidc = {
      authority,
      client_id: clientId,
      redirect_uri: `${window.location.origin}/auth/callback`,
      response_type: "code",
      scope: "openid email profile",
      // Cognito's discovery doc doesn't point at the Hosted UI domain, so wire it explicitly.
      metadata: {
        issuer: authority,
        authorization_endpoint: `https://${hostedDomain}/oauth2/authorize`,
        token_endpoint: `https://${hostedDomain}/oauth2/token`,
        userinfo_endpoint: `https://${hostedDomain}/oauth2/userInfo`,
        end_session_endpoint: `https://${hostedDomain}/logout`,
        jwks_uri: `${authority}/.well-known/jwks.json`,
      },
      // When federated to Identity Center, skip Cognito's IdP chooser and go
      // straight to the AWS access portal.
      ...(cfg.cognito.identityProvider
        ? { extraQueryParams: { identity_provider: cfg.cognito.identityProvider } }
        : {}),
      onSigninCallback: () => {
        void router.navigate({ to: "/" });
      },
    };

    root.render(
      <StrictMode>
        <ConfigContext.Provider value={cfg}>
          <AuthProvider {...oidc}>
            <RouterProvider router={router} />
          </AuthProvider>
        </ConfigContext.Provider>
      </StrictMode>,
    );
  })
  .catch((e) => {
    root.render(
      <pre style={{ color: "#f87171", padding: 24 }}>Failed to load /api/config: {String(e)}</pre>,
    );
  });
