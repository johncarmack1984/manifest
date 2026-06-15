import * as path from 'path';
import * as fs from 'fs';
import * as cdk from 'aws-cdk-lib';
import { Construct } from 'constructs';
import {
  aws_certificatemanager as acm,
  aws_cloudfront as cloudfront,
  aws_cloudfront_origins as origins,
  aws_cognito as cognito,
  aws_dynamodb as dynamodb,
  aws_iam as iam,
  aws_lambda as lambda,
  aws_logs as logs,
  aws_resourceexplorer2 as re,
  aws_route53 as route53,
  aws_route53_targets as targets,
  aws_s3 as s3,
  aws_s3_deployment as s3deploy,
} from 'aws-cdk-lib';
import { ManifestConfig, PRIMARY_REGION } from './config';

export interface ManifestStackProps extends cdk.StackProps {
  cfg: ManifestConfig;
}

// Build outputs (produced by `just api` / `just web`) with placeholders so that
// `cdk synth` works in CI without first building Rust + the SPA.
const API_BUILD = path.join(__dirname, '..', '..', 'api', 'target', 'lambda', 'manifest-api');
const WEB_BUILD = path.join(__dirname, '..', '..', 'web', 'dist');
const API_PLACEHOLDER = path.join(__dirname, '..', 'assets', 'lambda-placeholder');
const WEB_PLACEHOLDER = path.join(__dirname, '..', 'assets', 'web-placeholder');

export class ManifestStack extends cdk.Stack {
  constructor(scope: Construct, id: string, props: ManifestStackProps) {
    super(scope, id, props);
    const { cfg } = props;

    // ---------------------------------------------------------------------
    // Resource Explorer: AGGREGATOR index (this region) + a view over all.
    // ---------------------------------------------------------------------
    const index = new re.CfnIndex(this, 'Aggregator', { type: 'AGGREGATOR' });
    const view = new re.CfnView(this, 'AllView', {
      viewName: `${cfg.name}-all`,
      includedProperties: [{ name: 'tags' }],
    });
    view.addDependency(index);

    // ---------------------------------------------------------------------
    // Cost Explorer response cache (CE charges $0.01/request).
    // ---------------------------------------------------------------------
    const cache = new dynamodb.Table(this, 'Cache', {
      tableName: `${cfg.name}-cache`,
      partitionKey: { name: 'cache_key', type: dynamodb.AttributeType.STRING },
      billingMode: dynamodb.BillingMode.PAY_PER_REQUEST,
      timeToLiveAttribute: 'expires_at',
      removalPolicy: cdk.RemovalPolicy.DESTROY,
    });

    // ---------------------------------------------------------------------
    // Cognito Hosted UI (authorization code + PKCE). One admin-created user;
    // the SPA gets a JWT, the Axum API validates it against this pool's JWKS.
    // ---------------------------------------------------------------------
    const pool = new cognito.UserPool(this, 'Users', {
      userPoolName: cfg.name,
      signInAliases: { email: true },
      autoVerify: { email: true },
      selfSignUpEnabled: false,
      passwordPolicy: {
        minLength: 12,
        requireLowercase: true,
        requireUppercase: true,
        requireDigits: true,
        requireSymbols: true,
      },
      accountRecovery: cognito.AccountRecovery.EMAIL_ONLY,
      removalPolicy: cdk.RemovalPolicy.DESTROY,
    });

    pool.addDomain('HostedDomain', {
      cognitoDomain: { domainPrefix: cfg.cognitoDomainPrefix },
    });

    const client = pool.addClient('Spa', {
      userPoolClientName: 'spa',
      generateSecret: false,
      oAuth: {
        flows: { authorizationCodeGrant: true },
        scopes: [cognito.OAuthScope.OPENID, cognito.OAuthScope.EMAIL, cognito.OAuthScope.PROFILE],
        callbackUrls: [`https://${cfg.domainName}/auth/callback`],
        logoutUrls: [`https://${cfg.domainName}/`],
      },
      supportedIdentityProviders: [cognito.UserPoolClientIdentityProvider.COGNITO],
      authFlows: { userSrp: true },
      accessTokenValidity: cdk.Duration.minutes(60),
      idTokenValidity: cdk.Duration.minutes(60),
      refreshTokenValidity: cdk.Duration.days(30),
    });

    // The single dashboard user. Cognito emails a temporary password; set a
    // real one on first sign-in.
    new cognito.CfnUserPoolUser(this, 'Owner', {
      userPoolId: pool.userPoolId,
      username: cfg.ownerEmail,
      userAttributes: [
        { name: 'email', value: cfg.ownerEmail },
        { name: 'email_verified', value: 'true' },
      ],
    });

    // ---------------------------------------------------------------------
    // Lambda (Axum, arm64) + Function URL (IAM-auth, CloudFront-only via OAC).
    // ---------------------------------------------------------------------
    const apiLogs = new logs.LogGroup(this, 'ApiLogs', {
      logGroupName: `/aws/lambda/${cfg.name}`,
      retention: logs.RetentionDays.TWO_WEEKS,
      removalPolicy: cdk.RemovalPolicy.DESTROY,
    });

    const fn = new lambda.Function(this, 'Api', {
      functionName: cfg.name,
      runtime: lambda.Runtime.PROVIDED_AL2023,
      architecture: lambda.Architecture.ARM_64,
      handler: 'bootstrap',
      code: lambda.Code.fromAsset(asset(API_BUILD, API_PLACEHOLDER, this, 'api')),
      memorySize: 256,
      timeout: cdk.Duration.seconds(30),
      logGroup: apiLogs,
      environment: {
        RESOURCE_EXPLORER_VIEW_ARN: view.attrViewArn,
        CACHE_TABLE: cache.tableName,
        CACHE_TTL_SECONDS: String(cfg.cacheTtlSeconds),
        INDEXED_REGIONS: cfg.indexedRegions.join(','),
        APP_URL: `https://${cfg.domainName}`,
        ACCOUNT_ID: this.account,
        COGNITO_REGION: PRIMARY_REGION,
        COGNITO_USER_POOL_ID: pool.userPoolId,
        COGNITO_CLIENT_ID: client.userPoolClientId,
        COGNITO_HOSTED_DOMAIN: `${cfg.cognitoDomainPrefix}.auth.${PRIMARY_REGION}.amazoncognito.com`,
      },
    });

    // Least privilege: read cost + inventory, read/write only the cache table.
    fn.addToRolePolicy(
      new iam.PolicyStatement({
        sid: 'CostExplorerRead',
        actions: [
          'ce:GetCostAndUsage',
          'ce:GetCostAndUsageWithResources',
          'ce:GetCostForecast',
          'ce:GetDimensionValues',
          'ce:GetTags',
        ],
        resources: ['*'],
      }),
    );
    fn.addToRolePolicy(
      new iam.PolicyStatement({
        sid: 'ResourceExplorerRead',
        actions: [
          'resource-explorer-2:Search',
          'resource-explorer-2:GetView',
          'resource-explorer-2:ListViews',
          'resource-explorer-2:GetIndex',
          'resource-explorer-2:ListIndexes',
        ],
        resources: ['*'],
      }),
    );
    fn.addToRolePolicy(
      new iam.PolicyStatement({
        sid: 'Cache',
        actions: ['dynamodb:GetItem', 'dynamodb:PutItem'],
        resources: [cache.tableArn],
      }),
    );

    const fnUrl = fn.addFunctionUrl({ authType: lambda.FunctionUrlAuthType.AWS_IAM });

    // ---------------------------------------------------------------------
    // TLS cert + DNS (zone supplied by id/name so synth needs no AWS lookup).
    // ---------------------------------------------------------------------
    const zone = route53.HostedZone.fromHostedZoneAttributes(this, 'Zone', {
      hostedZoneId: cfg.hostedZoneId,
      zoneName: cfg.hostedZoneName,
    });
    const cert = new acm.Certificate(this, 'Cert', {
      domainName: cfg.domainName,
      validation: acm.CertificateValidation.fromDns(zone),
    });

    // ---------------------------------------------------------------------
    // Private SPA bucket (served only through CloudFront via OAC).
    // ---------------------------------------------------------------------
    const spa = new s3.Bucket(this, 'Spa', {
      bucketName: `${cfg.name}-web-${this.account}`,
      blockPublicAccess: s3.BlockPublicAccess.BLOCK_ALL,
      enforceSSL: true,
      removalPolicy: cdk.RemovalPolicy.DESTROY,
      autoDeleteObjects: true,
    });

    // ---------------------------------------------------------------------
    // CloudFront — SPA at /, Lambda Function URL at /api/*.
    // ---------------------------------------------------------------------
    const dist = new cloudfront.Distribution(this, 'Cdn', {
      comment: cfg.name,
      domainNames: [cfg.domainName],
      certificate: cert,
      defaultRootObject: 'index.html',
      priceClass: cloudfront.PriceClass.PRICE_CLASS_100,
      defaultBehavior: {
        origin: origins.S3BucketOrigin.withOriginAccessControl(spa),
        viewerProtocolPolicy: cloudfront.ViewerProtocolPolicy.REDIRECT_TO_HTTPS,
        cachePolicy: cloudfront.CachePolicy.CACHING_OPTIMIZED,
        allowedMethods: cloudfront.AllowedMethods.ALLOW_GET_HEAD_OPTIONS,
        compress: true,
      },
      additionalBehaviors: {
        '/api/*': {
          origin: origins.FunctionUrlOrigin.withOriginAccessControl(fnUrl),
          viewerProtocolPolicy: cloudfront.ViewerProtocolPolicy.REDIRECT_TO_HTTPS,
          cachePolicy: cloudfront.CachePolicy.CACHING_DISABLED,
          originRequestPolicy: cloudfront.OriginRequestPolicy.ALL_VIEWER_EXCEPT_HOST_HEADER,
          allowedMethods: cloudfront.AllowedMethods.ALLOW_ALL,
          compress: true,
        },
      },
      // SPA client-side routing: serve index.html for unknown paths.
      errorResponses: [
        { httpStatus: 403, responseHttpStatus: 200, responsePagePath: '/index.html' },
        { httpStatus: 404, responseHttpStatus: 200, responsePagePath: '/index.html' },
      ],
    });

    new route53.ARecord(this, 'AliasA', {
      zone,
      recordName: cfg.domainName,
      target: route53.RecordTarget.fromAlias(new targets.CloudFrontTarget(dist)),
    });
    new route53.AaaaRecord(this, 'AliasAAAA', {
      zone,
      recordName: cfg.domainName,
      target: route53.RecordTarget.fromAlias(new targets.CloudFrontTarget(dist)),
    });

    // Upload the built SPA and invalidate CloudFront in one step.
    new s3deploy.BucketDeployment(this, 'DeploySpa', {
      sources: [s3deploy.Source.asset(asset(WEB_BUILD, WEB_PLACEHOLDER, this, 'web'))],
      destinationBucket: spa,
      distribution: dist,
      distributionPaths: ['/*'],
    });

    // ---------------------------------------------------------------------
    // Outputs.
    // ---------------------------------------------------------------------
    new cdk.CfnOutput(this, 'Url', { value: `https://${cfg.domainName}` });
    new cdk.CfnOutput(this, 'SpaBucket', { value: spa.bucketName });
    new cdk.CfnOutput(this, 'DistributionId', { value: dist.distributionId });
    new cdk.CfnOutput(this, 'CognitoUserPoolId', { value: pool.userPoolId });
    new cdk.CfnOutput(this, 'CognitoClientId', { value: client.userPoolClientId });
    new cdk.CfnOutput(this, 'ResourceExplorerViewArn', { value: view.attrViewArn });
  }
}

/** Real build output if present, else a committed placeholder (so `cdk synth`
 *  works without building). Warns when the placeholder is used. */
function asset(real: string, placeholder: string, scope: Construct, label: string): string {
  if (fs.existsSync(real)) return real;
  cdk.Annotations.of(scope).addWarning(
    `${label} build output not found at ${real} — using placeholder. ` +
      'Run `just deploy` (which builds first) before deploying for real.',
  );
  return placeholder;
}
