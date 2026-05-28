/**
 * CDK Stack for run_002: File Processing Monitoring System
 *
 * Provisions the control-plane infrastructure that was previously created
 * inline by the original script.py:
 *   - S3 bucket  for storing processed files
 *   - SQS queue  for processing notifications
 *
 * Outputs (consumed by the refactored script.py):
 *   BucketName  → pass as --bucket    to script.py
 *   QueueUrl    → pass as --queue-url to script.py
 *   QueueName   → informational
 */

import * as cdk from 'aws-cdk-lib';
import * as s3 from 'aws-cdk-lib/aws-s3';
import * as sqs from 'aws-cdk-lib/aws-sqs';
import { Construct } from 'constructs';

export class FileMonitoringStack extends cdk.Stack {
  public readonly bucket: s3.Bucket;
  public readonly queue: sqs.Queue;

  constructor(scope: Construct, id: string, props?: cdk.StackProps) {
    super(scope, id, props);

    // ── S3 Bucket ─────────────────────────────────────────────────────────────
    // Mirrors the original create_s3_bucket() call.
    // DESTROY + autoDeleteObjects replaces the cleanup_resources() logic.
    this.bucket = new s3.Bucket(this, 'FileMonitorBucket', {
      // Keep the same naming convention as the original script so that the
      // existing IAM policy resource pattern (file-monitor-bucket-*) matches.
      removalPolicy: cdk.RemovalPolicy.DESTROY,
      autoDeleteObjects: true,
      // Sensible security defaults
      blockPublicAccess: s3.BlockPublicAccess.BLOCK_ALL,
      encryption: s3.BucketEncryption.S3_MANAGED,
      enforceSSL: true,
    });

    // ── SQS Queue ─────────────────────────────────────────────────────────────
    // Mirrors the original create_sqs_queue() call.
    // Attributes match the original:
    //   VisibilityTimeout     = 60 seconds
    //   MessageRetentionPeriod = 345600 seconds (4 days)
    this.queue = new sqs.Queue(this, 'FileMonitorQueue', {
      visibilityTimeout: cdk.Duration.seconds(60),
      retentionPeriod: cdk.Duration.seconds(345600), // 4 days
      removalPolicy: cdk.RemovalPolicy.DESTROY,
    });

    // ── Outputs ───────────────────────────────────────────────────────────────
    new cdk.CfnOutput(this, 'BucketName', {
      value: this.bucket.bucketName,
      description: 'S3 bucket name — pass as --bucket to script.py',
      exportName: `${id}-BucketName`,
    });

    new cdk.CfnOutput(this, 'QueueUrl', {
      value: this.queue.queueUrl,
      description: 'SQS queue URL — pass as --queue-url to script.py',
      exportName: `${id}-QueueUrl`,
    });

    new cdk.CfnOutput(this, 'QueueName', {
      value: this.queue.queueName,
      description: 'SQS queue name (informational)',
      exportName: `${id}-QueueName`,
    });
  }
}
