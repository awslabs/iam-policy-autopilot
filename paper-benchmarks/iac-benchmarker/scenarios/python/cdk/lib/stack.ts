import * as cdk from 'aws-cdk-lib';
import * as dynamodb from 'aws-cdk-lib/aws-dynamodb';
import * as iam from 'aws-cdk-lib/aws-iam';
import * as s3 from 'aws-cdk-lib/aws-s3';
import * as sns from 'aws-cdk-lib/aws-sns';
import * as sqs from 'aws-cdk-lib/aws-sqs';
import { Construct } from 'constructs';

export class DocumentPipelineStack extends cdk.Stack {
  constructor(scope: Construct, id: string, props?: cdk.StackProps) {
    super(scope, id, props);

    // ── S3 buckets ──────────────────────────────────────────────────────────
    const documentBucket = new s3.Bucket(this, 'DocumentBucket', {
      bucketName: `document-pipeline-uploads-${this.account}`,
      encryption: s3.BucketEncryption.S3_MANAGED,
      versioned: true,
      blockPublicAccess: s3.BlockPublicAccess.BLOCK_ALL,
      removalPolicy: cdk.RemovalPolicy.RETAIN,
    });

    // ── SQS queues ───────────────────────────────────────────────────────────
    const dlq = new sqs.Queue(this, 'DocumentDlq', {
      queueName: 'document-pipeline-dlq',
      retentionPeriod: cdk.Duration.days(14),
    });

    const documentQueue = new sqs.Queue(this, 'DocumentQueue', {
      queueName: 'document-pipeline-queue',
      visibilityTimeout: cdk.Duration.seconds(300),
      deadLetterQueue: { queue: dlq, maxReceiveCount: 3 },
    });

    // ── DynamoDB table ───────────────────────────────────────────────────────
    const metadataTable = new dynamodb.Table(this, 'MetadataTable', {
      tableName: 'document-metadata',
      partitionKey: { name: 'document_id', type: dynamodb.AttributeType.STRING },
      sortKey: { name: 'created_at', type: dynamodb.AttributeType.STRING },
      billingMode: dynamodb.BillingMode.PAY_PER_REQUEST,
      pointInTimeRecovery: true,
      removalPolicy: cdk.RemovalPolicy.RETAIN,
    });

    metadataTable.addGlobalSecondaryIndex({
      indexName: 'status-index',
      partitionKey: { name: 'status', type: dynamodb.AttributeType.STRING },
      sortKey: { name: 'updated_at', type: dynamodb.AttributeType.STRING },
    });

    // ── SNS topic ────────────────────────────────────────────────────────────
    const resultsTopic = new sns.Topic(this, 'ResultsTopic', {
      topicName: 'document-processing-results',
      displayName: 'Document Processing Results',
    });

    // ── IAM role for the worker process ─────────────────────────────────────
    const workerRole = new iam.Role(this, 'WorkerRole', {
      roleName: 'document-pipeline-worker',
      assumedBy: new iam.ServicePrincipal('ec2.amazonaws.com'),
      description: 'Role for the document processing worker',
    });

    documentBucket.grantReadWrite(workerRole);
    documentQueue.grantConsumeMessages(workerRole);
    metadataTable.grantReadWriteData(workerRole);
    resultsTopic.grantPublish(workerRole);

    // SSM read access for configuration secrets.
    workerRole.addToPolicy(new iam.PolicyStatement({
      actions: ['ssm:GetParameter', 'ssm:GetParameters', 'ssm:GetParametersByPath'],
      resources: [`arn:aws:ssm:${this.region}:${this.account}:parameter/document-pipeline/*`],
    }));

    // ── Outputs ──────────────────────────────────────────────────────────────
    new cdk.CfnOutput(this, 'DocumentBucketName', { value: documentBucket.bucketName });
    new cdk.CfnOutput(this, 'DocumentQueueUrl', { value: documentQueue.queueUrl });
    new cdk.CfnOutput(this, 'MetadataTableName', { value: metadataTable.tableName });
    new cdk.CfnOutput(this, 'ResultsTopicArn', { value: resultsTopic.topicArn });
    new cdk.CfnOutput(this, 'WorkerRoleArn', { value: workerRole.roleArn });
  }
}
