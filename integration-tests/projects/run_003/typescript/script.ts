#!/usr/bin/env node

import * as fs from 'fs';
import * as path from 'path';
import {
  S3Client,
  PutObjectCommand,
  ServerSideEncryption,
} from '@aws-sdk/client-s3';
import {
  SFNClient,
  StartExecutionCommand,
  DescribeExecutionCommand,
} from '@aws-sdk/client-sfn';
import {
  CloudWatchClient,
  PutMetricDataCommand,
  StandardUnit,
} from '@aws-sdk/client-cloudwatch';
import {
  STSClient,
  GetCallerIdentityCommand,
} from '@aws-sdk/client-sts';

// ── Config loading ─────────────────────────────────────────────────────────────

interface RunConfig {
  bucketName: string;
  kmsKeyId: string;
  kmsKeyArn: string;
  stateMachineArn: string;
  logGroupName: string;
  region: string;
}

function loadConfig(): RunConfig {
  const configPath = path.resolve(__dirname, '..', 'config.json');
  if (!fs.existsSync(configPath)) {
    throw new Error(
      `config.json not found at ${configPath}.\n` +
      'Deploy the CDK stack first:\n' +
      '  cd ../cdk && bash deploy.sh',
    );
  }
  return JSON.parse(fs.readFileSync(configPath, 'utf-8')) as RunConfig;
}

// ── Types ─────────────────────────────────────────────────────────────────────

interface SampleData {
  timestamp: number;
  data: string;
  processed: boolean;
}

interface ExecutionInput {
  bucket: string;
  timestamp: number;
}

interface PipelineResult {
  accountId: string;
  bucketName: string;
  dataKey: string;
  executionArn: string;
  executionStatus: string;
  stateMachineArn: string;
  region: string;
}

// ── Logger ────────────────────────────────────────────────────────────────────

class Logger {
  info(message: string): void {
    console.log(`${new Date().toISOString()} - INFO - ${message}`);
  }
  error(message: string): void {
    console.error(`${new Date().toISOString()} - ERROR - ${message}`);
  }
  warn(message: string): void {
    console.warn(`${new Date().toISOString()} - WARN - ${message}`);
  }
}

const logger = new Logger();

// ── Data-plane helpers ────────────────────────────────────────────────────────

async function getAwsAccountId(stsClient: STSClient): Promise<string> {
  const response = await stsClient.send(new GetCallerIdentityCommand({}));
  if (!response.Account) {
    throw new Error('No account ID returned from STS');
  }
  return response.Account;
}

async function uploadSampleData(
  s3Client: S3Client,
  bucketName: string,
  kmsKeyId: string,
  timestamp: number,
): Promise<string> {
  const key = `data/sample-${timestamp}.json`;
  const sampleData: SampleData = {
    timestamp,
    data: 'Sample data for processing pipeline',
    processed: false,
  };

  await s3Client.send(new PutObjectCommand({
    Bucket: bucketName,
    Key: key,
    Body: JSON.stringify(sampleData),
    ContentType: 'application/json',
    ServerSideEncryption: ServerSideEncryption.aws_kms,
    SSEKMSKeyId: kmsKeyId,
  }));

  logger.info(`Uploaded sample data to s3://${bucketName}/${key}`);
  return key;
}

async function startPipelineExecution(
  sfnClient: SFNClient,
  stateMachineArn: string,
  bucketName: string,
  timestamp: number,
): Promise<string> {
  const input: ExecutionInput = { bucket: bucketName, timestamp };
  const response = await sfnClient.send(new StartExecutionCommand({
    stateMachineArn,
    input: JSON.stringify(input),
  }));

  if (!response.executionArn) {
    throw new Error('No execution ARN returned from StartExecution');
  }
  logger.info(`Started execution: ${response.executionArn}`);
  return response.executionArn;
}

async function pollExecution(
  sfnClient: SFNClient,
  executionArn: string,
  timeoutSeconds: number,
): Promise<string> {
  const terminalStatuses = new Set(['SUCCEEDED', 'FAILED', 'TIMED_OUT', 'ABORTED']);
  const deadline = Date.now() + timeoutSeconds * 1000;

  while (Date.now() < deadline) {
    const response = await sfnClient.send(new DescribeExecutionCommand({ executionArn }));
    const status = response.status as string;
    logger.info(`Execution status: ${status}`);
    if (terminalStatuses.has(status)) {
      return status;
    }
    await new Promise(resolve => setTimeout(resolve, 5000));
  }

  throw new Error(`Execution did not reach terminal state within ${timeoutSeconds}s`);
}

async function putPipelineMetrics(cloudwatchClient: CloudWatchClient): Promise<void> {
  const namespace = 'DataProcessingPipeline';
  const now = new Date();

  await cloudwatchClient.send(new PutMetricDataCommand({
    Namespace: namespace,
    MetricData: [
      {
        MetricName: 'PipelineExecutions',
        Value: 1,
        Unit: StandardUnit.Count,
        Timestamp: now,
      },
      {
        MetricName: 'FilesProcessed',
        Value: 1,
        Unit: StandardUnit.Count,
        Timestamp: now,
      },
    ],
  }));

  logger.info(`Published metrics to CloudWatch namespace '${namespace}'`);
}

// ── Main logic ────────────────────────────────────────────────────────────────

async function runDataPipeline(cfg: RunConfig): Promise<PipelineResult> {
  const { bucketName, kmsKeyId, stateMachineArn, region } = cfg;

  const s3Client         = new S3Client({ region });
  const sfnClient        = new SFNClient({ region });
  const cloudwatchClient = new CloudWatchClient({ region });
  const stsClient        = new STSClient({ region });

  // 1. Get account ID
  logger.info('Getting AWS account information...');
  const accountId = await getAwsAccountId(stsClient);
  logger.info(`AWS Account ID: ${accountId}`);

  // 2. Upload sample data with KMS encryption
  const timestamp = Math.floor(Date.now() / 1000);
  logger.info('Uploading sample data to S3 with KMS encryption...');
  const dataKey = await uploadSampleData(s3Client, bucketName, kmsKeyId, timestamp);

  // 3. Start Step Functions execution
  logger.info('Starting Step Functions pipeline execution...');
  const executionArn = await startPipelineExecution(sfnClient, stateMachineArn, bucketName, timestamp);

  // 4. Poll for completion (60s timeout, 5s interval)
  logger.info('Polling for execution completion (timeout: 60s)...');
  const finalStatus = await pollExecution(sfnClient, executionArn, 60);
  logger.info(`Execution finished with status: ${finalStatus}`);

  // 5. Put custom CloudWatch metrics
  logger.info('Publishing custom CloudWatch metrics...');
  await putPipelineMetrics(cloudwatchClient);

  return {
    accountId,
    bucketName,
    dataKey,
    executionArn,
    executionStatus: finalStatus,
    stateMachineArn,
    region,
  };
}

// ── Entry point ───────────────────────────────────────────────────────────────

async function main(): Promise<void> {
  let cfg: RunConfig;
  try {
    cfg = loadConfig();
  } catch (error) {
    logger.error(`Configuration error: ${error}`);
    process.exit(1);
    return;
  }

  logger.info('Starting AWS Data Processing Pipeline...');
  logger.info(`Using bucket:        ${cfg.bucketName}`);
  logger.info(`Using KMS key:       ${cfg.kmsKeyId}`);
  logger.info(`Using state machine: ${cfg.stateMachineArn}`);
  logger.info(`Using region:        ${cfg.region}`);

  try {
    const result = await runDataPipeline(cfg);

    logger.info('='.repeat(60));
    logger.info('APPLICATION COMPLETED SUCCESSFULLY!');
    logger.info('='.repeat(60));
    logger.info('Resources used:');
    logger.info(`  - S3 Bucket:          ${result.bucketName}`);
    logger.info(`  - Data key:           ${result.dataKey}`);
    logger.info(`  - State Machine:      ${result.stateMachineArn}`);
    logger.info('  - CloudWatch Metrics: DataProcessingPipeline namespace');
    logger.info('Summary:');
    logger.info(`  - Execution ARN:      ${result.executionArn}`);
    logger.info(`  - Execution status:   ${result.executionStatus}`);
    logger.info('='.repeat(60));
    logger.info('To destroy infrastructure, run: cd ../cdk && npx cdk destroy');
  } catch (error) {
    logger.error(`Application failed: ${error}`);
    process.exit(1);
  }
}

if (require.main === module) {
  main().catch((error) => {
    console.error('Fatal error:', error);
    process.exit(1);
  });
}
