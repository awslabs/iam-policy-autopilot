import * as cdk from 'aws-cdk-lib';
import * as dynamodb from 'aws-cdk-lib/aws-dynamodb';
import * as iam from 'aws-cdk-lib/aws-iam';
import * as kinesis from 'aws-cdk-lib/aws-kinesis';
import * as secretsmanager from 'aws-cdk-lib/aws-secretsmanager';
import { Construct } from 'constructs';

export class FeatureFlagServiceStack extends cdk.Stack {
  constructor(scope: Construct, id: string, props?: cdk.StackProps) {
    super(scope, id, props);

    // ── DynamoDB table ───────────────────────────────────────────────────────
    const flagsTable = new dynamodb.Table(this, 'FeatureFlagsTable', {
      tableName: 'feature-flags',
      partitionKey: { name: 'flag_key', type: dynamodb.AttributeType.STRING },
      billingMode: dynamodb.BillingMode.PAY_PER_REQUEST,
      pointInTimeRecovery: true,
      removalPolicy: cdk.RemovalPolicy.RETAIN,
    });

    // ── Kinesis Data Stream ──────────────────────────────────────────────────
    const flagEventsStream = new kinesis.Stream(this, 'FlagEventsStream', {
      streamName: 'flag-events',
      shardCount: 2,
      retentionPeriod: cdk.Duration.hours(24),
      encryption: kinesis.StreamEncryption.MANAGED,
    });

    // ── Secrets Manager — Redis auth token ───────────────────────────────────
    const redisAuthSecret = new secretsmanager.Secret(this, 'RedisAuthSecret', {
      secretName: 'feature-flag-service/redis-auth',
      description: 'Redis AUTH token for the feature flag service cache',
      generateSecretString: {
        passwordLength: 32,
        excludePunctuation: true,
      },
    });

    // ── IAM role for the service ─────────────────────────────────────────────
    const serviceRole = new iam.Role(this, 'ServiceRole', {
      roleName: 'feature-flag-service',
      assumedBy: new iam.ServicePrincipal('ec2.amazonaws.com'),
      description: 'Role for the Feature Flag Service',
    });

    flagsTable.grantReadWriteData(serviceRole);
    flagEventsStream.grantWrite(serviceRole);
    redisAuthSecret.grantRead(serviceRole);

    serviceRole.addToPolicy(new iam.PolicyStatement({
      actions: [
        'appconfig:GetConfiguration',
        'appconfig:StartConfigurationSession',
        'appconfig:GetLatestConfiguration',
      ],
      resources: [`arn:aws:appconfig:${this.region}:${this.account}:*`],
    }));

    // ── Outputs ──────────────────────────────────────────────────────────────
    new cdk.CfnOutput(this, 'FlagsTableName', { value: flagsTable.tableName });
    new cdk.CfnOutput(this, 'FlagEventsStreamName', { value: flagEventsStream.streamName });
    new cdk.CfnOutput(this, 'RedisAuthSecretArn', { value: redisAuthSecret.secretArn });
    new cdk.CfnOutput(this, 'ServiceRoleArn', { value: serviceRole.roleArn });
  }
}
