import * as cdk from 'aws-cdk-lib';
import * as dynamodb from 'aws-cdk-lib/aws-dynamodb';
import * as iam from 'aws-cdk-lib/aws-iam';
import * as kms from 'aws-cdk-lib/aws-kms';
import * as logs from 'aws-cdk-lib/aws-logs';
import * as s3 from 'aws-cdk-lib/aws-s3';
import * as sns from 'aws-cdk-lib/aws-sns';
import * as sqs from 'aws-cdk-lib/aws-sqs';
import { Construct } from 'constructs';

export class AuditLogArchiverStack extends cdk.Stack {
  constructor(scope: Construct, id: string, props?: cdk.StackProps) {
    super(scope, id, props);

    // ── KMS key ──────────────────────────────────────────────────────────────
    const signingKey = new kms.Key(this, 'SigningKey', {
      alias: 'audit-log-signing-key',
      description: 'Asymmetric key for signing audit log records',
      keySpec: kms.KeySpec.RSA_2048,
      keyUsage: kms.KeyUsage.SIGN_VERIFY,
      enableKeyRotation: false, // asymmetric keys do not support auto-rotation
      removalPolicy: cdk.RemovalPolicy.RETAIN,
    });

    // ── SQS queues ───────────────────────────────────────────────────────────
    const dlq = new sqs.Queue(this, 'AuditDlq', {
      queueName: 'audit-events-dlq',
      retentionPeriod: cdk.Duration.days(14),
    });

    const auditQueue = new sqs.Queue(this, 'AuditQueue', {
      queueName: 'audit-events',
      visibilityTimeout: cdk.Duration.seconds(300),
      deadLetterQueue: { queue: dlq, maxReceiveCount: 3 },
      encryption: sqs.QueueEncryption.KMS_MANAGED,
    });

    // ── DynamoDB table ───────────────────────────────────────────────────────
    const userProfilesTable = new dynamodb.Table(this, 'UserProfilesTable', {
      tableName: 'user-profiles',
      partitionKey: { name: 'user_id', type: dynamodb.AttributeType.STRING },
      billingMode: dynamodb.BillingMode.PAY_PER_REQUEST,
      pointInTimeRecovery: true,
      removalPolicy: cdk.RemovalPolicy.RETAIN,
    });

    // ── S3 archive bucket ────────────────────────────────────────────────────
    const archiveBucket = new s3.Bucket(this, 'ArchiveBucket', {
      bucketName: `audit-archive-${this.account}`,
      encryption: s3.BucketEncryption.S3_MANAGED,
      blockPublicAccess: s3.BlockPublicAccess.BLOCK_ALL,
      lifecycleRules: [
        {
          id: 'move-to-glacier',
          transitions: [{ storageClass: s3.StorageClass.GLACIER, transitionAfter: cdk.Duration.days(30) }],
          expiration: cdk.Duration.days(2555), // 7 years
        },
      ],
      removalPolicy: cdk.RemovalPolicy.RETAIN,
    });

    // ── SNS topic ────────────────────────────────────────────────────────────
    const complianceTopic = new sns.Topic(this, 'ComplianceTopic', {
      topicName: 'compliance-alerts',
      displayName: 'Audit Compliance Alerts',
    });

    // ── CloudWatch log group ─────────────────────────────────────────────────
    const logGroup = new logs.LogGroup(this, 'AuditLogGroup', {
      logGroupName: '/audit-log-archiver',
      retention: logs.RetentionDays.THREE_YEARS,
      removalPolicy: cdk.RemovalPolicy.RETAIN,
    });

    // ── IAM role ─────────────────────────────────────────────────────────────
    const processorRole = new iam.Role(this, 'ProcessorRole', {
      roleName: 'audit-log-processor',
      assumedBy: new iam.ServicePrincipal('ec2.amazonaws.com'),
    });

    auditQueue.grantConsumeMessages(processorRole);
    userProfilesTable.grantReadWriteData(processorRole);
    archiveBucket.grantWrite(processorRole);
    complianceTopic.grantPublish(processorRole);
    signingKey.grantSign(processorRole);
    signingKey.grantVerify(processorRole);
    signingKey.grant(processorRole, 'kms:GenerateDataKey', 'kms:DescribeKey');

    processorRole.addToPolicy(new iam.PolicyStatement({
      actions: ['logs:CreateLogStream', 'logs:PutLogEvents', 'logs:DescribeLogStreams'],
      resources: [logGroup.logGroupArn + ':*'],
    }));

    processorRole.addToPolicy(new iam.PolicyStatement({
      actions: ['ssm:GetParameter', 'ssm:GetParameters'],
      resources: [`arn:aws:ssm:${this.region}:${this.account}:parameter/audit-log-archiver/*`],
    }));

    // ── Outputs ──────────────────────────────────────────────────────────────
    new cdk.CfnOutput(this, 'AuditQueueUrl', { value: auditQueue.queueUrl });
    new cdk.CfnOutput(this, 'UserProfilesTableName', { value: userProfilesTable.tableName });
    new cdk.CfnOutput(this, 'ArchiveBucketName', { value: archiveBucket.bucketName });
    new cdk.CfnOutput(this, 'KmsKeyId', { value: signingKey.keyId });
    new cdk.CfnOutput(this, 'ComplianceTopicArn', { value: complianceTopic.topicArn });
    new cdk.CfnOutput(this, 'LogGroupName', { value: logGroup.logGroupName });
    new cdk.CfnOutput(this, 'ProcessorRoleArn', { value: processorRole.roleArn });
  }
}
