/**
 * CDK Stack for run_006: Secure Document Management System
 *
 * Provisions the control-plane infrastructure:
 *   - KMS key + alias    for document encryption (alias/secure-docs-key-run003-849526af)
 *   - S3 bucket          for secure document storage (KMS-encrypted, versioning enabled)
 *   - DynamoDB table     for document metadata (document-metadata-run003-849526af)
 *   - CloudWatch log group  /aws/secure-document-manager/run003-849526af
 *
 * Outputs (consumed by the refactored scripts via config.json):
 *   BucketName    → S3 bucket name
 *   TableName     → DynamoDB table name
 *   KmsKeyId      → KMS key ID
 *   KmsKeyArn     → KMS key ARN
 *   KmsAlias      → KMS key alias
 *   LogGroupName  → CloudWatch log group name
 *   Region        → deployment region
 */

import * as cdk from 'aws-cdk-lib';
import * as s3 from 'aws-cdk-lib/aws-s3';
import * as kms from 'aws-cdk-lib/aws-kms';
import * as dynamodb from 'aws-cdk-lib/aws-dynamodb';
import * as logs from 'aws-cdk-lib/aws-logs';
import { Construct } from 'constructs';

export class SecureDocumentStack extends cdk.Stack {
  public readonly bucket: s3.Bucket;
  public readonly kmsKey: kms.Key;
  public readonly table: dynamodb.Table;
  public readonly logGroup: logs.LogGroup;

  constructor(scope: Construct, id: string, props?: cdk.StackProps) {
    super(scope, id, props);

    // ── KMS Key ───────────────────────────────────────────────────────────────
    // Customer-managed key for S3 object encryption.
    this.kmsKey = new kms.Key(this, 'SecureDocsKey', {
      description: 'KMS key for secure document management (run_006)',
      removalPolicy: cdk.RemovalPolicy.DESTROY,
      pendingWindow: cdk.Duration.days(7),
      enableKeyRotation: true,
    });

    // ── KMS Alias ─────────────────────────────────────────────────────────────
    new kms.Alias(this, 'SecureDocsKeyAlias', {
      aliasName: 'alias/secure-docs-key-run003-849526af',
      targetKey: this.kmsKey,
      removalPolicy: cdk.RemovalPolicy.DESTROY,
    });

    // ── S3 Bucket ─────────────────────────────────────────────────────────────
    // Secure document storage bucket with KMS encryption and versioning.
    // DESTROY + autoDeleteObjects replaces cleanup logic.
    this.bucket = new s3.Bucket(this, 'SecureDocsBucket', {
      encryption: s3.BucketEncryption.KMS,
      encryptionKey: this.kmsKey,
      versioned: true,
      removalPolicy: cdk.RemovalPolicy.DESTROY,
      autoDeleteObjects: true,
      blockPublicAccess: s3.BlockPublicAccess.BLOCK_ALL,
      enforceSSL: true,
    });

    // ── DynamoDB Table ────────────────────────────────────────────────────────
    // Stores document metadata with document_id as the partition key.
    this.table = new dynamodb.Table(this, 'DocumentMetadataTable', {
      tableName: 'document-metadata-run003-849526af',
      partitionKey: {
        name: 'document_id',
        type: dynamodb.AttributeType.STRING,
      },
      billingMode: dynamodb.BillingMode.PAY_PER_REQUEST,
      removalPolicy: cdk.RemovalPolicy.DESTROY,
    });

    // ── CloudWatch Log Group ──────────────────────────────────────────────────
    // Receives audit logs for document upload/download operations.
    this.logGroup = new logs.LogGroup(this, 'SecureDocumentLogGroup', {
      logGroupName: '/aws/secure-document-manager/run003-849526af',
      removalPolicy: cdk.RemovalPolicy.DESTROY,
      retention: logs.RetentionDays.ONE_DAY,
    });

    // ── Outputs ───────────────────────────────────────────────────────────────
    new cdk.CfnOutput(this, 'BucketName', {
      value: this.bucket.bucketName,
      description: 'S3 bucket name for secure document storage',
      exportName: `${id}-BucketName`,
    });

    new cdk.CfnOutput(this, 'TableName', {
      value: this.table.tableName,
      description: 'DynamoDB table name for document metadata',
      exportName: `${id}-TableName`,
    });

    new cdk.CfnOutput(this, 'KmsKeyId', {
      value: this.kmsKey.keyId,
      description: 'KMS key ID for document encryption',
      exportName: `${id}-KmsKeyId`,
    });

    new cdk.CfnOutput(this, 'KmsKeyArn', {
      value: this.kmsKey.keyArn,
      description: 'KMS key ARN for document encryption',
      exportName: `${id}-KmsKeyArn`,
    });

    new cdk.CfnOutput(this, 'KmsAlias', {
      value: 'alias/secure-docs-key-run003-849526af',
      description: 'KMS key alias for document encryption',
      exportName: `${id}-KmsAlias`,
    });

    new cdk.CfnOutput(this, 'LogGroupName', {
      value: this.logGroup.logGroupName,
      description: 'CloudWatch log group name for document operations',
      exportName: `${id}-LogGroupName`,
    });

    new cdk.CfnOutput(this, 'Region', {
      value: cdk.Stack.of(this).region,
      description: 'Deployment region',
      exportName: `${id}-Region`,
    });
  }
}
