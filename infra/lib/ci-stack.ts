import * as cdk from 'aws-cdk-lib';
import { Construct } from 'constructs';
import { aws_iam as iam } from 'aws-cdk-lib';

export interface ManifestCiStackProps extends cdk.StackProps {
  /** Base resource name (matches the main stack's). */
  name: string;
  /** GitHub repo allowed to assume the role, as "owner/repo". */
  githubRepo: string;
  /** ARN of an existing GitHub OIDC provider to reuse (only one is allowed per
   *  account); empty to create one. */
  githubOidcArn: string;
  /** CDK bootstrap qualifier whose roles the deploy role may assume (default hnb659fds). */
  cdkQualifier: string;
}

/**
 * GitHub Actions CI deploy role (OIDC) — the role `.github/workflows/deploy.yml`
 * assumes via a short-lived GitHub OIDC token, so the repo holds no long-lived AWS
 * keys. It can run `cdk deploy` (by assuming the account's CDK bootstrap roles) and
 * nothing else. Deployed once, out of band, by `just ci-role` — never by
 * `cdk deploy --all`.
 */
export class ManifestCiStack extends cdk.Stack {
  constructor(scope: Construct, id: string, props: ManifestCiStackProps) {
    super(scope, id, props);
    const { name, githubRepo, githubOidcArn, cdkQualifier } = props;
    const GH = 'token.actions.githubusercontent.com';

    const provider = githubOidcArn
      ? iam.OpenIdConnectProvider.fromOpenIdConnectProviderArn(this, 'GitHubOidc', githubOidcArn)
      : new iam.OpenIdConnectProvider(this, 'GitHubOidc', {
          url: `https://${GH}`,
          clientIds: ['sts.amazonaws.com'],
        });

    const role = new iam.Role(this, 'DeployRole', {
      roleName: `${name}-ci-deploy`,
      description: 'GitHub Actions OIDC deploy role for the manifest dashboard.',
      maxSessionDuration: cdk.Duration.hours(1),
      assumedBy: new iam.OpenIdConnectPrincipal(provider, {
        StringEquals: {
          [`${GH}:aud`]: 'sts.amazonaws.com',
          // Only this repo's workflows on the main branch may assume the role.
          [`${GH}:sub`]: `repo:${githubRepo}:ref:refs/heads/main`,
        },
      }),
    });

    // `cdk deploy` does its work by assuming the account's CDK bootstrap roles, so
    // that assume-role is the only privilege the deploy role needs.
    role.addToPolicy(
      new iam.PolicyStatement({
        sid: 'AssumeCdkBootstrapRoles',
        actions: ['sts:AssumeRole'],
        resources: [`arn:aws:iam::${this.account}:role/cdk-${cdkQualifier}-*`],
      }),
    );

    new cdk.CfnOutput(this, 'CiDeployRoleArn', {
      description: 'Set this as the repo variable AWS_DEPLOY_ROLE_ARN',
      value: role.roleArn,
    });
  }
}
