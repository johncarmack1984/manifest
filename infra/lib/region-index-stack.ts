import * as cdk from 'aws-cdk-lib';
import { Construct } from 'constructs';
import { aws_resourceexplorer2 as re } from 'aws-cdk-lib';

/**
 * A Resource Explorer LOCAL index for one region. The AGGREGATOR index (in the
 * primary region, created by ManifestStack) searches across every region that
 * has an index like this one.
 */
export class RegionIndexStack extends cdk.Stack {
  constructor(scope: Construct, id: string, props: cdk.StackProps) {
    super(scope, id, props);
    new re.CfnIndex(this, 'LocalIndex', { type: 'LOCAL' });
  }
}
