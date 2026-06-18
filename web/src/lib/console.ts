// Best-effort AWS console deep-link for an inventory row. Returns null when there's
// no good link for that type (the caller hides the icon). Links target the resource's
// own region; cross-account resources open in whatever account you're signed into, so
// they're only direct for the dashboard's own account.

interface Linkable {
  type: string;
  region: string;
  arn: string;
  name: string;
}

/** Trailing id segment of an ARN (after the last ":" or "/"). */
function tail(arn: string, fallback: string): string {
  const parts = arn.split(/[:/]/);
  return parts[parts.length - 1] || fallback;
}

export function consoleUrl(r: Linkable): string | null {
  const region = r.region && r.region !== "global" ? r.region : "us-east-1";
  const id = tail(r.arn, r.name);
  const svc = (s: string) => `https://${region}.console.aws.amazon.com/${s}/home?region=${region}`;
  const iam = (p: string) => `https://us-east-1.console.aws.amazon.com/iam/home#/${p}`;

  switch (r.type) {
    case "s3:bucket":
      return `https://s3.console.aws.amazon.com/s3/buckets/${r.name}?region=${region}`;
    case "lambda:function":
      return `${svc("lambda")}#/functions/${r.name}`;
    case "iam:role":
      return iam(`roles/details/${r.name}`);
    case "iam:user":
      return iam(`users/details/${r.name}`);
    case "iam:policy":
      return iam("policies");
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
      return `https://us-east-1.console.aws.amazon.com/cloudfront/v4/home#/distributions/${id}`;
    case "route53:hostedzone":
      return `https://us-east-1.console.aws.amazon.com/route53/v2/hostedzones#ListRecordSets/${id}`;
    case "route53:domain":
      return "https://us-east-1.console.aws.amazon.com/route53/domains/home#/";
    case "ses:identity":
      return `${svc("ses")}#/verified-identities/${r.name}`;
    case "apigateway:restapi":
      return `${svc("apigateway")}#/apis/${id}/resources`;
    case "states:statemachine":
      return `${svc("states")}#/statemachines/view/${r.arn}`;
    case "events:rule":
      return `${svc("events")}#/eventbus/default/rules/${r.name}`;
    case "cognito-idp:userpool":
      return `${svc("cognito")}#/pool/${id}/details`;
    case "ec2:instance":
      return `${svc("ec2")}#InstanceDetails:instanceId=${id}`;
    default:
      // Universal fallback: open the resource in Resource Explorer by ARN.
      return r.arn
        ? `https://${region}.console.aws.amazon.com/resource-explorer/home?region=${region}#/search?query=${encodeURIComponent(r.arn)}`
        : null;
  }
}
