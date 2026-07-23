/**
 * Manifest configuration, read from environment variables (see infra/.env.example).
 * No personal values are baked in — every deployment supplies its own.
 */

// CloudFront ACM certs and the Cost Explorer API are us-east-1-only, so the
// primary stack (CloudFront, Cognito, Lambda, DynamoDB, RE aggregator) lives there.
export const PRIMARY_REGION = 'us-east-1';

export interface ManifestConfig {
  /** Base name for resources (Lambda, DynamoDB table, bucket prefix, tags). */
  name: string;
  /** Regions with a Resource Explorer index. Cost views still cover ALL regions;
   *  spend in a region absent here is flagged as a blind spot. */
  indexedRegions: string[];
  /** How long Cost Explorer responses are cached in DynamoDB. */
  cacheTtlSeconds: number;
  /** Public hostname for the dashboard, e.g. manifest.example.com. */
  domainName: string;
  /** Route53 hosted zone id that owns domainName. */
  hostedZoneId: string;
  /** The hosted zone's name (no trailing dot), e.g. example.com. */
  hostedZoneName: string;
  /** Globally-unique prefix for the Cognito Hosted UI domain. */
  cognitoDomainPrefix: string;
  /** Email of the single dashboard user (admin-created in the pool). Ignored
   *  when samlMetadataUrl is set (users then come from Identity Center). */
  ownerEmail: string;
  /** Where spend-anomaly alerts are emailed. Defaults to ownerEmail. Only empty
   *  when neither it nor ownerEmail is set (e.g. Identity Center federation with
   *  no MANIFEST_ALERT_EMAIL) — the topic is then created with no subscription. */
  alertEmail: string;
  /** Anomaly alerting: a service/account day fires only when it clears BOTH an
   *  absolute daily-increase floor (dollars) and a relative jump (percent), vs a
   *  spike-robust trailing baseline of this many days. */
  anomalyMinDollars: number;
  anomalyPct: number;
  anomalyBaselineDays: number;
  /** When the daily anomaly scan runs (an EventBridge cron expression, UTC). */
  anomalyScheduleCron: string;
  /** Optional: IAM Identity Center SAML app metadata URL. When set, sign-in
   *  federates to Identity Center (AWS SSO) and no Cognito-local user is made. */
  samlMetadataUrl: string;
  /** Whether to create the Resource Explorer AGGREGATOR index (+ per-region LOCAL
   *  indexes). Set false to reuse an aggregator the account already has — only one
   *  is allowed per account/region. The manifest-owned view is created either way. */
  createAggregator: boolean;
  /** IAM role name the Lambda assumes in each org member account to inventory it
   *  (deploy it there with `just member-deploy`). Inventory is per-account even
   *  though cost is org-wide. Empty string disables cross-account inventory. */
  memberInventoryRole: string;
  /** Management/payer account id that member accounts trust (so the Lambda there
   *  can assume the inventory role). Only needed to deploy the member-account stack. */
  payerAccountId: string;
  /** GitHub repo ("owner/repo") allowed to assume the CI deploy role. Only used by
   *  `just ci-role` (the continuous-deployment setup). */
  githubRepo: string;
  /** ARN of an existing GitHub OIDC provider to reuse (one per account); empty to
   *  create one. Only used by `just ci-role`. */
  githubOidcArn: string;
  /** CDK bootstrap qualifier whose roles the CI deploy role may assume (default hnb659fds). */
  cdkQualifier: string;
}

export function loadConfig(): ManifestConfig {
  const e = process.env;
  const cfg: ManifestConfig = {
    name: e.MANIFEST_NAME || 'manifest',
    indexedRegions: (e.MANIFEST_INDEXED_REGIONS || PRIMARY_REGION)
      .split(',')
      .map((s) => s.trim())
      .filter(Boolean),
    cacheTtlSeconds: Number(e.MANIFEST_CACHE_TTL_SECONDS || 3600),
    domainName: e.MANIFEST_DOMAIN_NAME || '',
    hostedZoneId: e.MANIFEST_HOSTED_ZONE_ID || '',
    hostedZoneName: e.MANIFEST_HOSTED_ZONE_NAME || '',
    cognitoDomainPrefix: e.MANIFEST_COGNITO_DOMAIN_PREFIX || '',
    ownerEmail: e.MANIFEST_OWNER_EMAIL || '',
    // Alerts default to the dashboard owner; MANIFEST_ALERT_EMAIL overrides. Unset
    // (or empty — an unset CD repo variable arrives as "") falls back to the owner.
    alertEmail: e.MANIFEST_ALERT_EMAIL || e.MANIFEST_OWNER_EMAIL || '',
    anomalyMinDollars: Number(e.MANIFEST_ANOMALY_MIN_DOLLARS || 5),
    anomalyPct: Number(e.MANIFEST_ANOMALY_PCT || 50),
    anomalyBaselineDays: Number(e.MANIFEST_ANOMALY_BASELINE_DAYS || 14),
    anomalyScheduleCron: e.MANIFEST_ANOMALY_SCHEDULE || 'cron(0 13 * * ? *)',
    samlMetadataUrl: e.MANIFEST_SAML_METADATA_URL || '',
    createAggregator: e.MANIFEST_CREATE_AGGREGATOR !== 'false',
    memberInventoryRole:
      e.MANIFEST_MEMBER_ROLE === undefined ? 'ManifestInventoryRole' : e.MANIFEST_MEMBER_ROLE,
    payerAccountId: e.MANIFEST_PAYER_ACCOUNT || '',
    githubRepo: e.MANIFEST_GITHUB_REPO || '',
    githubOidcArn: e.MANIFEST_GITHUB_OIDC_ARN || '',
    cdkQualifier: e.MANIFEST_CDK_QUALIFIER || 'hnb659fds',
  };

  const required: Record<string, string> = {
    MANIFEST_DOMAIN_NAME: cfg.domainName,
    MANIFEST_HOSTED_ZONE_ID: cfg.hostedZoneId,
    MANIFEST_HOSTED_ZONE_NAME: cfg.hostedZoneName,
    MANIFEST_COGNITO_DOMAIN_PREFIX: cfg.cognitoDomainPrefix,
    // Only needed for the default Cognito-local login; Identity Center
    // federation (samlMetadataUrl) provides users instead.
    ...(cfg.samlMetadataUrl ? {} : { MANIFEST_OWNER_EMAIL: cfg.ownerEmail }),
  };
  const missing = Object.entries(required)
    .filter(([, v]) => !v)
    .map(([k]) => k);
  if (missing.length) {
    throw new Error(
      `Missing required config: ${missing.join(', ')}.\n` +
        'Copy infra/.env.example to infra/.env and fill these in (see README).',
    );
  }

  // The aggregator's view returns resources only from regions that have an
  // index; the primary region always has one (the AGGREGATOR).
  if (!cfg.indexedRegions.includes(PRIMARY_REGION)) {
    cfg.indexedRegions.unshift(PRIMARY_REGION);
  }
  return cfg;
}
