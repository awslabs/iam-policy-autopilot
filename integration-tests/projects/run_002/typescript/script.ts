#!/usr/bin/env node

import * as fs from 'fs';
import * as path from 'path';
import {
  S3Client,
  PutObjectCommand,
} from '@aws-sdk/client-s3';
import {
  SQSClient,
  SendMessageCommand,
  ReceiveMessageCommand,
  DeleteMessageCommand,
} from '@aws-sdk/client-sqs';
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
  queueUrl: string;
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

interface FileInfo {
  filename: string;
  size: number;
  type: string;
}

interface ProcessedFileContent {
  filename: string;
  processed_at: string;
  size: number;
  type: string;
  processed_by: string;
}

interface SQSMessageBody {
  action: string;
  filename: string;
  bucket: string;
  size: number;
  timestamp: string;
  account_id: string;
}

interface ProcessingResult {
  bucket_name: string;
  queue_url: string;
  processed_files: number;
  total_size: number;
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

async function uploadFileToS3(
  s3Client: S3Client,
  bucketName: string,
  fileContent: string,
  fileKey: string,
): Promise<void> {
  await s3Client.send(new PutObjectCommand({
    Bucket: bucketName,
    Key: fileKey,
    Body: fileContent,
    ContentType: 'application/json',
  }));
}

async function sendSqsMessage(
  sqsClient: SQSClient,
  queueUrl: string,
  messageBody: SQSMessageBody,
): Promise<string> {
  const response = await sqsClient.send(new SendMessageCommand({
    QueueUrl: queueUrl,
    MessageBody: JSON.stringify(messageBody),
  }));
  if (!response.MessageId) {
    throw new Error('No message ID returned');
  }
  return response.MessageId;
}

async function receiveSqsMessages(
  sqsClient: SQSClient,
  queueUrl: string,
  maxMessages: number = 10,
): Promise<any[]> {
  const response = await sqsClient.send(new ReceiveMessageCommand({
    QueueUrl: queueUrl,
    MaxNumberOfMessages: maxMessages,
    WaitTimeSeconds: 5,
  }));
  return response.Messages ?? [];
}

async function deleteSqsMessage(
  sqsClient: SQSClient,
  queueUrl: string,
  receiptHandle: string,
): Promise<void> {
  await sqsClient.send(new DeleteMessageCommand({
    QueueUrl: queueUrl,
    ReceiptHandle: receiptHandle,
  }));
}

async function putCloudWatchMetric(
  cloudwatchClient: CloudWatchClient,
  namespace: string,
  metricName: string,
  value: number,
  unit: StandardUnit = StandardUnit.Count,
): Promise<void> {
  await cloudwatchClient.send(new PutMetricDataCommand({
    Namespace: namespace,
    MetricData: [
      {
        MetricName: metricName,
        Value: value,
        Unit: unit,
        Timestamp: new Date(),
      },
    ],
  }));
}

// ── Main logic ────────────────────────────────────────────────────────────────

async function processFileMonitoringSystem(
  cfg: RunConfig,
): Promise<ProcessingResult> {

  const { bucketName, queueUrl, region } = cfg;

  const s3Client = new S3Client({ region });
  const sqsClient = new SQSClient({ region });
  const cloudwatchClient = new CloudWatchClient({ region });
  const stsClient = new STSClient({ region });

  logger.info('Getting AWS account information...');
  const accountId = await getAwsAccountId(stsClient);
  logger.info(`Using AWS Account ID: ${accountId}`);

  const filesToProcess: FileInfo[] = [
    { filename: 'data1.json', size: 1024, type: 'json' },
    { filename: 'data2.json', size: 2048, type: 'json' },
    { filename: 'data3.json', size: 512,  type: 'json' },
  ];

  let processedFiles = 0;
  let totalSize = 0;

  for (const fileInfo of filesToProcess) {
    const fileContent: ProcessedFileContent = {
      filename: fileInfo.filename,
      processed_at: new Date().toISOString(),
      size: fileInfo.size,
      type: fileInfo.type,
      processed_by: 'file-monitoring-system',
    };

    logger.info(`Uploading ${fileInfo.filename} to S3...`);
    await uploadFileToS3(s3Client, bucketName, JSON.stringify(fileContent, null, 2), fileInfo.filename);

    const sqsMessage: SQSMessageBody = {
      action: 'file_processed',
      filename: fileInfo.filename,
      bucket: bucketName,
      size: fileInfo.size,
      timestamp: new Date().toISOString(),
      account_id: accountId,
    };

    logger.info('Sending processing notification to SQS...');
    const messageId = await sendSqsMessage(sqsClient, queueUrl, sqsMessage);
    logger.info(`SQS message sent with ID: ${messageId}`);

    processedFiles += 1;
    totalSize += fileInfo.size;

    logger.info('Sending metrics to CloudWatch...');
    await putCloudWatchMetric(cloudwatchClient, 'FileProcessing', 'FilesProcessed', 1);
    await putCloudWatchMetric(cloudwatchClient, 'FileProcessing', 'BytesProcessed', fileInfo.size, StandardUnit.Bytes);

    await new Promise(resolve => setTimeout(resolve, 1000));
  }

  logger.info('Reading processing notifications from SQS...');
  const messages = await receiveSqsMessages(sqsClient, queueUrl);

  for (const message of messages) {
    const messageBody = JSON.parse(message.Body) as SQSMessageBody;
    logger.info(`Processing notification: ${messageBody.filename} (${messageBody.size} bytes)`);

    await deleteSqsMessage(sqsClient, queueUrl, message.ReceiptHandle);
    logger.info('Notification processed and removed from queue');
  }

  logger.info('Sending summary metrics to CloudWatch...');
  await putCloudWatchMetric(cloudwatchClient, 'FileProcessing', 'TotalFilesProcessed', processedFiles);
  await putCloudWatchMetric(cloudwatchClient, 'FileProcessing', 'TotalBytesProcessed', totalSize, StandardUnit.Bytes);

  logger.info('File processing monitoring completed!');
  logger.info(`Total files processed: ${processedFiles}`);
  logger.info(`Total bytes processed: ${totalSize}`);
  logger.info(`S3 bucket:   ${bucketName}`);
  logger.info(`SQS queue URL: ${queueUrl}`);

  return {
    bucket_name: bucketName,
    queue_url: queueUrl,
    processed_files: processedFiles,
    total_size: totalSize,
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
    return; // satisfies TypeScript definite-assignment analysis
  }

  logger.info('Starting AWS File Processing Monitoring System...');
  logger.info(`Using bucket:    ${cfg.bucketName}`);
  logger.info(`Using queue URL: ${cfg.queueUrl}`);
  logger.info(`Using region:    ${cfg.region}`);

  try {
    const result = await processFileMonitoringSystem(cfg);

    logger.info('='.repeat(60));
    logger.info('APPLICATION COMPLETED SUCCESSFULLY!');
    logger.info('='.repeat(60));
    logger.info('Resources used:');
    logger.info(`  - S3 Bucket:          ${result.bucket_name}`);
    logger.info(`  - SQS Queue URL:      ${result.queue_url}`);
    logger.info('  - CloudWatch Metrics: FileProcessing namespace');
    logger.info('Summary:');
    logger.info(`  - Files processed:    ${result.processed_files}`);
    logger.info(`  - Total bytes:        ${result.total_size}`);
    logger.info('='.repeat(60));
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
