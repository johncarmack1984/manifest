import * as cdk from 'aws-cdk-lib';
import { Construct } from 'constructs';
import { aws_iam as iam } from 'aws-cdk-lib';

export interface ManifestMemberStackProps extends cdk.StackProps {
  /** Role name to create — must match the payer deployment's MEMBER_INVENTORY_ROLE. */
  roleName: string;
  /** Management/payer account whose manifest Lambda assumes this role. */
  payerAccountId: string;
}

/**
 * Deployed INTO each org member account (`just member-deploy` with that account's
 * credentials), NOT into the payer account. It creates the least-privilege role the
 * manifest Lambda assumes to read that account's inventory — read-only Resource
 * Explorer + ACM, nothing else.
 *
 * Prerequisite: the member account must have Resource Explorer enabled (an index +
 * default view) in each region you want covered. Cost stays org-wide via the payer's
 * Cost Explorer regardless; this only governs the per-account *inventory* sweep.
 */
export class ManifestMemberStack extends cdk.Stack {
  constructor(scope: Construct, id: string, props: ManifestMemberStackProps) {
    super(scope, id, props);
    const { roleName, payerAccountId } = props;

    new iam.Role(this, 'InventoryRole', {
      roleName,
      // The payer account is trusted; the actual caller is gated by the narrow
      // sts:AssumeRole grant on the manifest Lambda's own role.
      assumedBy: new iam.AccountPrincipal(payerAccountId),
      description: 'Read-only inventory access for the manifest dashboard in the payer account.',
      inlinePolicies: {
        inventory: new iam.PolicyDocument({
          statements: [
            new iam.PolicyStatement({
              sid: 'ResourceExplorerRead',
              actions: [
                'resource-explorer-2:Search',
                'resource-explorer-2:GetView',
                'resource-explorer-2:ListViews',
                'resource-explorer-2:GetDefaultView',
                'resource-explorer-2:GetIndex',
                'resource-explorer-2:ListIndexes',
              ],
              resources: ['*'],
            }),
            new iam.PolicyStatement({
              sid: 'AcmRead',
              actions: ['acm:DescribeCertificate'],
              resources: ['*'],
            }),
            // Mirrors the payer Lambda's read-only grants, so member-account rows get
            // the same display-name resolution and "created on" lookups.
            new iam.PolicyStatement({
              sid: 'GlobalNameRead',
              actions: ['cloudfront:ListDistributions', 'route53:ListHostedZones'],
              resources: ['*'],
            }),
            new iam.PolicyStatement({
              sid: 'CreatedOnRead',
              actions: [
                'ec2:DescribeVolumes',
                'ec2:DescribeInstances',
                'ec2:DescribeKeyPairs',
                'ec2:DescribeLaunchTemplates',
                's3:ListAllMyBuckets',
                'iam:GetRole',
                'iam:GetUser',
                'logs:DescribeLogGroups',
                'dynamodb:DescribeTable',
                'secretsmanager:DescribeSecret',
                'ecr:DescribeRepositories',
                'cognito-idp:DescribeUserPool',
              ],
              resources: ['*'],
            }),
          ],
        }),
      },
    });

    new cdk.CfnOutput(this, 'RoleArn', {
      value: `arn:aws:iam::${this.account}:role/${roleName}`,
    });
  }
}
