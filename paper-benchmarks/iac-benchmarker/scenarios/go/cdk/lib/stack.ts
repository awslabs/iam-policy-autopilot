import * as cdk from 'aws-cdk-lib';
import * as dynamodb from 'aws-cdk-lib/aws-dynamodb';
import * as iam from 'aws-cdk-lib/aws-iam';
import * as kinesis from 'aws-cdk-lib/aws-kinesis';
import * as logs from 'aws-cdk-lib/aws-logs';
import * as s3 from 'aws-cdk-lib/aws-s3';
import { Construct } from 'constructs';

export class MetricsAggregationStack extends cdk.Stack {
  constructor(scope: Construct, id: string, props?: cdk.StackProps) {
    super(scope, id, props);

    // ── Kinesis Data Stream ──────────────────────────────────────────────────
    const metricsStream = new kinesis.Stream(this, 'MetricsStream', {
      streamName: 'metrics-stream',
      shardCount: 4,
      retentionPeriod: cdk.Duration.hours(24),
      encryption: kinesis.StreamEncryption.MANAGED,
    });

    // ── S3 archive bucket ────────────────────────────────────────────────────
    const archiveBucket = new s3.Bucket(this, 'ArchiveBucket', {
      bucketName: `metrics-archive-${this.account}`,
      encryption: s3.BucketEncryption.S3_MANAGED,
      lifecycleRules: [
        {
          id: 'archive-to-glacier',
          transitions: [
            { storageClass: s3.StorageClass.GLACIER, transitionAfter: cdk.Duration.days(90) },
          ],
          expiration: cdk.Duration.days(365),
        },
      ],
      blockPublicAccess: s3.BlockPublicAccess.BLOCK_ALL,
      removalPolicy: cdk.RemovalPolicy.RETAIN,
    });

    // ── DynamoDB table ───────────────────────────────────────────────────────
    const metricsTable = new dynamodb.Table(this, 'MetricsTable', {
      tableName: 'metrics-aggregates',
      partitionKey: { name: 'metric_name', type: dynamodb.AttributeType.STRING },
      sortKey: { name: 'window_start', type: dynamodb.AttributeType.STRING },
      billingMode: dynamodb.BillingMode.PAY_PER_REQUEST,
      timeToLiveAttribute: 'ttl',
      pointInTimeRecovery: true,
      removalPolicy: cdk.RemovalPolicy.RETAIN,
    });

    // ── CloudWatch log group ─────────────────────────────────────────────────
    const logGroup = new logs.LogGroup(this, 'ServiceLogs', {
      logGroupName: '/metrics-aggregation-service',
      retention: logs.RetentionDays.ONE_MONTH,
      removalPolicy: cdk.RemovalPolicy.DESTROY,
    });

    // ── IAM role for the consumer process ────────────────────────────────────
    const consumerRole = new iam.Role(this, 'ConsumerRole', {
      roleName: 'metrics-aggregation-consumer',
      assumedBy: new iam.ServicePrincipal('ec2.amazonaws.com'),
      description: 'Role for the metrics aggregation consumer',
    });

    metricsStream.grantRead(consumerRole);
    archiveBucket.grantWrite(consumerRole);
    metricsTable.grantReadWriteData(consumerRole);

    consumerRole.addToPolicy(new iam.PolicyStatement({
      actions: [
        'cloudwatch:PutMetricData',
        'logs:CreateLogStream',
        'logs:PutLogEvents',
      ],
      resources: ['*'],
    }));

    consumerRole.addToPolicy(new iam.PolicyStatement({
      actions: [
        'appconfig:GetConfiguration',
        'appconfig:StartConfigurationSession',
        'appconfig:GetLatestConfiguration',
      ],
      resources: [`arn:aws:appconfig:${this.region}:${this.account}:*`],
    }));

    // ── Outputs ──────────────────────────────────────────────────────────────
    new cdk.CfnOutput(this, 'StreamName', { value: metricsStream.streamName });
    new cdk.CfnOutput(this, 'ArchiveBucketName', { value: archiveBucket.bucketName });
    new cdk.CfnOutput(this, 'MetricsTableName', { value: metricsTable.tableName });
    new cdk.CfnOutput(this, 'LogGroupName', { value: logGroup.logGroupName });
    new cdk.CfnOutput(this, 'ConsumerRoleArn', { value: consumerRole.roleArn });
  }
}
