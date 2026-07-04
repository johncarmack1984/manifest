// Best-effort AWS console deep-link for an inventory row. Returns null when there's
// no good link for that type (the caller hides the icon). Links target the resource's
// own region; cross-account resources open in whatever account you're signed into, so
// they're only direct for the dashboard's own account.

interface Linkable {
  type: string;
  region: string;
  arn: string;
  name: string;
  service?: string;
  account?: string;
}

/** Trailing id segment of an ARN (after the last ":" or "/"). */
function tail(arn: string, fallback: string): string {
  const parts = arn.split(/[:/]/);
  return parts[parts.length - 1] || fallback;
}

/** Services whose console lives under a different slug than their ARN service id. */
const SERVICE_CONSOLE: Record<string, string> = {
  logs: "cloudwatch",
  "cognito-idp": "cognito",
  elasticloadbalancing: "ec2",
  "resource-explorer-2": "resource-explorer",
};

export function consoleUrl(r: Linkable): string | null {
  const region = r.region && r.region !== "global" ? r.region : "us-east-1";
  const id = tail(r.arn, r.name);
  const svc = (s: string) => `https://${region}.console.aws.amazon.com/${s}/home?region=${region}`;
  const iam = (p: string) => `https://us-east-1.console.aws.amazon.com/iam/home#/${p}`;
  const cloudfront = (p: string) => `https://us-east-1.console.aws.amazon.com/cloudfront/v4/home#/${p}`;

  switch (r.type) {
    case "s3:bucket":
      return `https://s3.console.aws.amazon.com/s3/buckets/${r.name}?region=${region}`;
    case "lambda:function":
      return `${svc("lambda")}#/functions/${r.name}`;
    case "iam:role":
      return iam(`roles/details/${r.name}`);
    case "iam:user":
      return iam(`users/details/${r.name}`);
    case "iam:group":
      return iam(`groups/details/${r.name}`);
    case "iam:policy":
      return iam("policies");
    case "iam:instance-profile":
      return iam("roles");
    case "iam:oidc-provider":
    case "iam:saml-provider":
      return iam("identity_providers");
    case "logs:log-group":
      return `${svc("cloudwatch")}#logsV2:log-groups/log-group/${encodeURIComponent(r.name)}`;
    case "cloudwatch:alarm":
      return `${svc("cloudwatch")}#alarmsV2:alarm/${encodeURIComponent(r.name)}`;
    case "dynamodb:table":
      return `${svc("dynamodbv2")}#table?name=${r.name}`;
    case "acm:certificate":
      return `${svc("acm")}#/certificates/${id}`;
    case "sns:topic":
      return `${svc("sns")}#/topic/${r.arn}`;
    case "sqs:queue":
      return `${svc("sqs")}#/queues`;
    case "cloudfront:distribution":
      return cloudfront(`distributions/${id}`);
    case "cloudfront:function":
      return cloudfront(`functions/${r.name}`);
    case "cloudfront:origin-access-control":
    case "cloudfront:origin-access-identity":
      return cloudfront("originAccess");
    case "cloudfront:cache-policy":
      return cloudfront("policies/cache");
    case "cloudfront:origin-request-policy":
      return cloudfront("policies/origin-request");
    case "cloudfront:response-headers-policy":
      return cloudfront("policies/response-headers");
    case "route53:hostedzone":
      return `https://us-east-1.console.aws.amazon.com/route53/v2/hostedzones#ListRecordSets/${id}`;
    case "route53:domain":
      return "https://us-east-1.console.aws.amazon.com/route53/domains/home#/";
    case "ses:identity":
      return `${svc("ses")}#/verified-identities/${r.name}`;
    case "ses:configuration-set":
      return `${svc("ses")}#/configuration-sets/${r.name}`;
    case "apigateway:restapi":
      return `${svc("apigateway")}#/apis/${id}/resources`;
    case "states:statemachine":
      return `${svc("states")}#/statemachines/view/${r.arn}`;
    case "events:rule":
      return `${svc("events")}#/eventbus/default/rules/${r.name}`;
    case "events:connection":
      return `${svc("events")}#/connections`;
    case "events:api-destination":
      return `${svc("events")}#/apidestinations`;
    case "cognito-idp:userpool":
      return `${svc("cognito")}#/pool/${id}/details`;
    case "ec2:instance":
      return `${svc("ec2")}#InstanceDetails:instanceId=${id}`;
    case "ec2:volume":
      return `${svc("ec2")}#VolumeDetails:volumeId=${id}`;
    case "ec2:network-interface":
      return `${svc("ec2")}#NetworkInterfaces:search=${id}`;
    case "ec2:key-pair":
      return `${svc("ec2")}#KeyPairs:search=${id}`;
    case "ec2:launch-template":
      return `${svc("ec2")}#LaunchTemplateDetails:launchTemplateId=${id}`;
    case "ec2:elastic-ip":
      return `${svc("ec2")}#Addresses:search=${id}`;
    case "autoscaling:autoScalingGroup":
      return `${svc("ec2")}#AutoScalingGroupDetails:id=${encodeURIComponent(r.name)};view=details`;
    case "ecs:cluster":
      return `https://${region}.console.aws.amazon.com/ecs/v2/clusters/${r.name}?region=${region}`;
    case "ecr:repository":
      return r.account
        ? `https://${region}.console.aws.amazon.com/ecr/repositories/private/${r.account}/${r.name}?region=${region}`
        : svc("ecr");
    case "batch:compute-environment":
      return `${svc("batch")}#compute-environments`;
    case "batch:job-queue":
      return `${svc("batch")}#queues`;
    case "batch:job-definition":
      return `${svc("batch")}#job-definition`;
    case "secretsmanager:secret":
      return `https://${region}.console.aws.amazon.com/secretsmanager/secret?name=${encodeURIComponent(r.name)}&region=${region}`;
    case "ssm:parameter": {
      const p = r.name.startsWith("/") ? r.name : `/${r.name}`;
      return `https://${region}.console.aws.amazon.com/systems-manager/parameters${p}/description?region=${region}`;
    }
    default:
      // Universal fallback: the owning service's console home in the right region.
      // (The Resource Explorer console stopped accepting freeform/ARN queries, so
      // deep-searching there errors; a service home never does.)
      return r.service ? svc(SERVICE_CONSOLE[r.service] ?? r.service) : null;
  }
}
