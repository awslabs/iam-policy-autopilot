#!/usr/bin/env node

import * as fs from 'fs';
import * as path from 'path';
import * as crypto from 'crypto';
import {
  S3Client,
  PutObjectCommand,
  GetObjectCommand,
  ServerSideEncryption,
} from '@aws-sdk/client-s3';
import {
  DynamoDBClient,
} from '@aws-sdk/client-dynamodb';
import {
  DynamoDBDocumentClient,
  PutCommand,
  ScanCommand,
  GetCommand,
} from '@aws-sdk/lib-dynamodb';
import {
  CloudWatchLogsClient,
  CreateLogStreamCommand,
  PutLogEventsCommand,
} from '@aws-sdk/client-cloudwatch-logs';
import {
  STSClient,
  GetCallerIdentityCommand,
} from '@aws-sdk/client-sts';
import { Readable } from 'stream';

// ── Config loading ─────────────────────────────────────────────────────────────

interface RunConfig {
  bucketName: string;
  tableName: string;
  kmsKeyId: string;
  kmsKeyArn: string;
  kmsAlias: string;
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

// ── Helpers ───────────────────────────────────────────────────────────────────

function sha256Hex(data: Buffer | string): string {
  return crypto.createHash('sha256').update(data).digest('hex');
}

async function streamToBuffer(stream: Readable): Promise<Buffer> {
  const chunks: Buffer[] = [];
  for await (const chunk of stream) {
    chunks.push(Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk));
  }
  return Buffer.concat(chunks);
}

// ── Data-plane helpers ────────────────────────────────────────────────────────

async function getAwsAccountId(stsClient: STSClient): Promise<string> {
  const response = await stsClient.send(new GetCallerIdentityCommand({}));
  if (!response.Account) {
    throw new Error('No account ID returned from STS');
  }
  return response.Account;
}

interface UploadResult {
  documentId: string;
  s3Key: string;
  fileHash: string;
}

async function uploadDocument(
  s3Client: S3Client,
  docClient: DynamoDBDocumentClient,
  bucketName: string,
  tableName: string,
  kmsKeyId: string,
  fileContent: Buffer,
  documentName: string,
): Promise<UploadResult> {
  const fileHash = sha256Hex(fileContent);
  const rawId = sha256Hex(`${documentName}_${Date.now()}`);
  const documentId = rawId.substring(0, 16);
  const s3Key = `documents/${documentId}/${documentName}`;

  // S3 PutObject with KMS encryption
  await s3Client.send(new PutObjectCommand({
    Bucket: bucketName,
    Key: s3Key,
    Body: fileContent,
    ServerSideEncryption: ServerSideEncryption.aws_kms,
    SSEKMSKeyId: kmsKeyId,
    Metadata: {
      'document-id': documentId,
      'original-name': documentName,
    },
  }));
  logger.info(`Uploaded to s3://${bucketName}/${s3Key}`);

  // DynamoDB PutItem — store metadata
  await docClient.send(new PutCommand({
    TableName: tableName,
    Item: {
      document_id:       documentId,
      document_name:     documentName,
      s3_bucket:         bucketName,
      s3_key:            s3Key,
      file_hash:         fileHash,
      file_size:         fileContent.length,
      upload_timestamp:  new Date().toISOString(),
      status:            'active',
    },
  }));
  logger.info(`Stored metadata in DynamoDB for document_id=${documentId}`);

  return { documentId, s3Key, fileHash };
}

async function logOperation(
  cwlClient: CloudWatchLogsClient,
  logGroupName: string,
  operation: string,
  documentId: string,
  documentName: string,
  status: string,
): Promise<void> {
  const logStreamName = `document-operations-${new Date().toISOString().slice(0, 10)}`;

  // CreateLogStream (ignore ResourceAlreadyExistsException)
  try {
    await cwlClient.send(new CreateLogStreamCommand({
      logGroupName,
      logStreamName,
    }));
  } catch (err: unknown) {
    const e = err as { name?: string };
    if (e.name !== 'ResourceAlreadyExistsException') {
      throw err;
    }
  }

  const logEntry = JSON.stringify({
    timestamp:     new Date().toISOString(),
    operation,
    document_id:   documentId,
    document_name: documentName,
    status,
  });

  await cwlClient.send(new PutLogEventsCommand({
    logGroupName,
    logStreamName,
    logEvents: [
      {
        timestamp: Date.now(),
        message:   logEntry,
      },
    ],
  }));
  logger.info(`Logged ${operation} operation to CloudWatch`);
}

async function listDocuments(
  docClient: DynamoDBDocumentClient,
  tableName: string,
): Promise<Record<string, unknown>[]> {
  const response = await docClient.send(new ScanCommand({ TableName: tableName }));
  const docs = (response.Items ?? []) as Record<string, unknown>[];
  logger.info(`Found ${docs.length} document(s) in DynamoDB`);
  return docs;
}

async function downloadDocument(
  s3Client: S3Client,
  docClient: DynamoDBDocumentClient,
  bucketName: string,
  tableName: string,
  documentId: string,
  downloadPath: string,
): Promise<string> {
  // DynamoDB GetItem — fetch metadata
  const getResult = await docClient.send(new GetCommand({
    TableName: tableName,
    Key: { document_id: documentId },
  }));

  if (!getResult.Item) {
    throw new Error(`Document not found: ${documentId}`);
  }

  const s3Key      = getResult.Item['s3_key'] as string;
  const storedHash = getResult.Item['file_hash'] as string;
  const docName    = getResult.Item['document_name'] as string;

  // S3 GetObject
  const getObjResult = await s3Client.send(new GetObjectCommand({
    Bucket: bucketName,
    Key:    s3Key,
  }));

  if (!getObjResult.Body) {
    throw new Error('Empty response body from S3 GetObject');
  }

  const fileContent = await streamToBuffer(getObjResult.Body as Readable);

  // Integrity check
  const fileHash = sha256Hex(fileContent);
  if (fileHash !== storedHash) {
    throw new Error('File integrity check failed');
  }

  fs.writeFileSync(downloadPath, fileContent);
  logger.info(`Downloaded document to ${downloadPath}`);
  return docName;
}

// ── Main logic ────────────────────────────────────────────────────────────────

interface DemoResult {
  accountId: string;
  documentId: string;
  documentsCount: number;
}

async function runDemo(cfg: RunConfig): Promise<DemoResult> {
  const { bucketName, tableName, kmsKeyId, logGroupName, region } = cfg;

  const s3Client     = new S3Client({ region });
  const dbClient     = new DynamoDBClient({ region });
  const docClient    = DynamoDBDocumentClient.from(dbClient);
  const cwlClient    = new CloudWatchLogsClient({ region });
  const stsClient    = new STSClient({ region });

  // 1. STS GetCallerIdentity
  logger.info('Getting AWS account information...');
  const accountId = await getAwsAccountId(stsClient);
  logger.info(`AWS Account ID: ${accountId}`);

  // 2. Create sample document
  const sampleContent = Buffer.from(
    'This is a sample document for testing the secure document management system.',
    'utf-8',
  );
  const samplePath = '/tmp/sample_document.txt';
  fs.writeFileSync(samplePath, sampleContent);
  logger.info(`Created sample document at ${samplePath}`);

  // 3. S3 PutObject + DynamoDB PutItem
  logger.info('Uploading document...');
  const uploadResult = await uploadDocument(
    s3Client, docClient, bucketName, tableName, kmsKeyId,
    sampleContent, 'sample_document.txt',
  );

  // 4. CloudWatch Logs — log UPLOAD
  await logOperation(cwlClient, logGroupName,
    'UPLOAD', uploadResult.documentId, 'sample_document.txt', 'SUCCESS');

  // 5. DynamoDB Scan — list all documents
  const docs = await listDocuments(docClient, tableName);

  // 6. S3 GetObject + DynamoDB GetItem — download
  logger.info('Downloading document...');
  const downloadPath = '/tmp/downloaded_sample.txt';
  const docName = await downloadDocument(
    s3Client, docClient, bucketName, tableName,
    uploadResult.documentId, downloadPath,
  );

  // 7. CloudWatch Logs — log DOWNLOAD
  await logOperation(cwlClient, logGroupName,
    'DOWNLOAD', uploadResult.documentId, docName, 'SUCCESS');

  return {
    accountId,
    documentId:     uploadResult.documentId,
    documentsCount: docs.length,
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

  logger.info('Starting Secure Document Management System...');
  logger.info(`Using bucket:    ${cfg.bucketName}`);
  logger.info(`Using table:     ${cfg.tableName}`);
  logger.info(`Using KMS key:   ${cfg.kmsKeyId}`);
  logger.info(`Using log group: ${cfg.logGroupName}`);
  logger.info(`Using region:    ${cfg.region}`);

  try {
    const result = await runDemo(cfg);

    logger.info('='.repeat(60));
    logger.info('APPLICATION COMPLETED SUCCESSFULLY!');
    logger.info('='.repeat(60));
    logger.info('Resources used:');
    logger.info(`  - S3 Bucket:   ${cfg.bucketName}`);
    logger.info(`  - DynamoDB:    ${cfg.tableName}`);
    logger.info(`  - Log Group:   ${cfg.logGroupName}`);
    logger.info('Summary:');
    logger.info(`  - Document ID:    ${result.documentId}`);
    logger.info(`  - Total docs:     ${result.documentsCount}`);
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
