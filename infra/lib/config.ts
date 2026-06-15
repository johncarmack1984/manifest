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
  /** Optional: IAM Identity Center SAML app metadata URL. When set, sign-in
   *  federates to Identity Center (AWS SSO) and no Cognito-local user is made. */
  samlMetadataUrl: string;
  /** Optional: reuse an existing Resource Explorer view (ARN) instead of creating
   *  an aggregator index + view. Set when the account already has Resource Explorer
   *  (only one aggregator/index is allowed per account/region). */
  resourceExplorerViewArn: string;
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
    samlMetadataUrl: e.MANIFEST_SAML_METADATA_URL || '',
    resourceExplorerViewArn: e.MANIFEST_RESOURCE_EXPLORER_VIEW_ARN || '',
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
