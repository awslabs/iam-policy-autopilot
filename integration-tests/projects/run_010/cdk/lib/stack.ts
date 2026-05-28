import * as cdk from 'aws-cdk-lib';
import * as lambda from 'aws-cdk-lib/aws-lambda';
import * as logs from 'aws-cdk-lib/aws-logs';
import { Construct } from 'constructs';

export class DeploymentMonitoringStack extends cdk.Stack {
  public readonly fn: lambda.Function;
  public readonly logGroup: logs.LogGroup;

  constructor(scope: Construct, id: string, props?: cdk.StackProps) {
    super(scope, id, props);

    // Pre-create the log group so it gets DESTROY removal policy
    this.logGroup = new logs.LogGroup(this, 'LambdaLogGroup', {
      logGroupName: `/aws/lambda/deployment-monitor-run005-9a05981d`,
      removalPolicy: cdk.RemovalPolicy.DESTROY,
      retention: logs.RetentionDays.ONE_DAY,
    });

    this.fn = new lambda.Function(this, 'DeploymentMonitorFn', {
      functionName: 'deployment-monitor-run005-9a05981d',
      runtime: lambda.Runtime.PYTHON_3_14,
      handler: 'index.handler',
      code: lambda.Code.fromInline(`
def handler(event, context):
    return {"statusCode": 200, "body": "deployment monitoring test"}
`),
      logGroup: this.logGroup,
    });
    this.fn.applyRemovalPolicy(cdk.RemovalPolicy.DESTROY);

    new cdk.CfnOutput(this, 'FunctionName', {
      value: this.fn.functionName,
      exportName: `${id}-FunctionName`,
    });

    new cdk.CfnOutput(this, 'LogGroupName', {
      value: this.logGroup.logGroupName,
      exportName: `${id}-LogGroupName`,
    });

    new cdk.CfnOutput(this, 'Region', {
      value: cdk.Stack.of(this).region,
      exportName: `${id}-Region`,
    });
  }
}
