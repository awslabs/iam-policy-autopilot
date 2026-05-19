/**
 * CDK Stack for run_003: AWS Data Processing Pipeline
 *
 * Provisions the control-plane infrastructure:
 *   - S3 bucket          for data storage
 *   - KMS key + alias    for encryption (alias/data-pipeline-run002-7beb16a2)
 *   - IAM role           for Step Functions (AWSLambdaRole + CloudWatchLogsFullAccess)
 *   - CloudWatch log group  /aws/stepfunctions/pipeline-*
 *   - Step Functions state machine  (STANDARD type, with logging)
 *
 * Outputs (consumed by the refactored scripts via config.json):
 *   BucketName       → S3 bucket name
 *   KmsKeyId         → KMS key ID
 *   KmsKeyArn        → KMS key ARN
 *   StateMachineArn  → Step Functions state machine ARN
 *   LogGroupName     → CloudWatch log group name
 *   Region           → deployment region
 */

import * as cdk from 'aws-cdk-lib';
import * as s3 from 'aws-cdk-lib/aws-s3';
import * as kms from 'aws-cdk-lib/aws-kms';
import * as iam from 'aws-cdk-lib/aws-iam';
import * as logs from 'aws-cdk-lib/aws-logs';
import * as sfn from 'aws-cdk-lib/aws-stepfunctions';
import { Construct } from 'constructs';

export class DataPipelineStack extends cdk.Stack {
  public readonly bucket: s3.Bucket;
  public readonly kmsKey: kms.Key;
  public readonly stateMachine: sfn.CfnStateMachine;
  public readonly logGroup: logs.LogGroup;

  constructor(scope: Construct, id: string, props?: cdk.StackProps) {
    super(scope, id, props);

    // ── S3 Bucket ─────────────────────────────────────────────────────────────
    // Data storage bucket for the pipeline.
    // DESTROY + autoDeleteObjects replaces cleanup logic.
    this.bucket = new s3.Bucket(this, 'DataPipelineBucket', {
      removalPolicy: cdk.RemovalPolicy.DESTROY,
      autoDeleteObjects: true,
      blockPublicAccess: s3.BlockPublicAccess.BLOCK_ALL,
      enforceSSL: true,
    });

    // ── KMS Key ───────────────────────────────────────────────────────────────
    // Customer-managed key for S3 object encryption.
    this.kmsKey = new kms.Key(this, 'DataPipelineKey', {
      description: 'KMS key for data pipeline encryption (run_003)',
      removalPolicy: cdk.RemovalPolicy.DESTROY,
      pendingWindow: cdk.Duration.days(7),
      enableKeyRotation: true,
    });

    // ── KMS Alias ─────────────────────────────────────────────────────────────
    new kms.Alias(this, 'DataPipelineKeyAlias', {
      aliasName: 'alias/data-pipeline-run002-7beb16a2',
      targetKey: this.kmsKey,
      removalPolicy: cdk.RemovalPolicy.DESTROY,
    });

    // ── CloudWatch Log Group ──────────────────────────────────────────────────
    // Receives execution logs from the Step Functions state machine.
    this.logGroup = new logs.LogGroup(this, 'PipelineLogGroup', {
      logGroupName: `/aws/stepfunctions/pipeline-run002-7beb16a2`,
      removalPolicy: cdk.RemovalPolicy.DESTROY,
      retention: logs.RetentionDays.ONE_WEEK,
    });

    // ── IAM Role for Step Functions ───────────────────────────────────────────
    // Allows Step Functions to call S3 (listObjects) and CloudWatch Logs
    // (putLogEvents) on behalf of the state machine.
    const sfnRole = new iam.Role(this, 'StepFunctionsRole', {
      assumedBy: new iam.ServicePrincipal('states.amazonaws.com'),
      description: 'Execution role for the data pipeline state machine',
      managedPolicies: [
        iam.ManagedPolicy.fromAwsManagedPolicyName('service-role/AWSLambdaRole'),
        iam.ManagedPolicy.fromAwsManagedPolicyName('CloudWatchLogsFullAccess'),
      ],
    });

    // Grant the role access to the S3 bucket and KMS key so the state machine
    // can call s3:ListObjects on the bucket.
    this.bucket.grantRead(sfnRole);
    this.kmsKey.grantDecrypt(sfnRole);

    // ── Step Functions State Machine ──────────────────────────────────────────
    // STANDARD type state machine that:
    //   1. Lists objects in the S3 bucket (aws-sdk integration)
    //   2. Puts a log event to CloudWatch Logs (aws-sdk integration)
    const stateMachineDefinition = {
      Comment: 'Data processing pipeline (run_003)',
      StartAt: 'ListS3Objects',
      States: {
        ListS3Objects: {
          Type: 'Task',
          Resource: 'arn:aws:states:::aws-sdk:s3:listObjects',
          Parameters: {
            'Bucket.$': '$.bucket',
          },
          ResultPath: '$.s3Result',
          Next: 'PutLogEvent',
        },
        PutLogEvent: {
          Type: 'Task',
          Resource: 'arn:aws:states:::aws-sdk:cloudwatchlogs:putLogEvents',
          Parameters: {
            LogGroupName: this.logGroup.logGroupName,
            LogStreamName: 'pipeline-executions',
            LogEvents: [
              {
                'Timestamp.$': '$.timestamp',
                Message: 'Pipeline execution completed',
              },
            ],
          },
          ResultPath: '$.logResult',
          End: true,
        },
      },
    };

    this.stateMachine = new sfn.CfnStateMachine(this, 'DataPipelineStateMachine', {
      stateMachineType: 'STANDARD',
      roleArn: sfnRole.roleArn,
      definitionString: JSON.stringify(stateMachineDefinition),
      loggingConfiguration: {
        level: 'ALL',
        includeExecutionData: true,
        destinations: [
          {
            cloudWatchLogsLogGroup: {
              logGroupArn: this.logGroup.logGroupArn,
            },
          },
        ],
      },
    });

    // ── Outputs ───────────────────────────────────────────────────────────────
    new cdk.CfnOutput(this, 'BucketName', {
      value: this.bucket.bucketName,
      description: 'S3 bucket name for data storage',
      exportName: `${id}-BucketName`,
    });

    new cdk.CfnOutput(this, 'KmsKeyId', {
      value: this.kmsKey.keyId,
      description: 'KMS key ID for data encryption',
      exportName: `${id}-KmsKeyId`,
    });

    new cdk.CfnOutput(this, 'KmsKeyArn', {
      value: this.kmsKey.keyArn,
      description: 'KMS key ARN for data encryption',
      exportName: `${id}-KmsKeyArn`,
    });

    new cdk.CfnOutput(this, 'StateMachineArn', {
      value: this.stateMachine.ref,
      description: 'Step Functions state machine ARN',
      exportName: `${id}-StateMachineArn`,
    });

    new cdk.CfnOutput(this, 'LogGroupName', {
      value: this.logGroup.logGroupName,
      description: 'CloudWatch log group name for pipeline executions',
      exportName: `${id}-LogGroupName`,
    });

    new cdk.CfnOutput(this, 'Region', {
      value: cdk.Stack.of(this).region,
      description: 'Deployment region',
      exportName: `${id}-Region`,
    });
  }
}
