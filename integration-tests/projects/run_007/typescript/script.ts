#!/usr/bin/env node

import * as fs from 'fs';
import * as path from 'path';
import {
  S3Client,
  PutObjectCommand,
} from '@aws-sdk/client-s3';
import {
  SESClient,
  SendEmailCommand,
} from '@aws-sdk/client-ses';
import {
  STSClient,
  GetCallerIdentityCommand,
} from '@aws-sdk/client-sts';

// ── Config loading ─────────────────────────────────────────────────────────────

interface RunConfig {
  bucketName: string;
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

// ── Data-plane helpers ────────────────────────────────────────────────────────

async function uploadSampleData(s3Client: S3Client, bucketName: string): Promise<void> {
  const sampleData = {
    timestamp: Date.now() / 1000,
    system_metrics: {
      cpu_usage:    Math.random() * 80 + 10,
      memory_usage: Math.random() * 60 + 20,
      disk_usage:   Math.random() * 40 + 30,
    },
    application_metrics: {
      requests_per_second: Math.floor(Math.random() * 900) + 100,
      error_rate:          Math.random() * 4.9 + 0.1,
      response_time:       Math.random() * 400 + 100,
    },
  };

  const key = `monitoring-data/${Math.floor(Date.now() / 1000)}.json`;

  await s3Client.send(new PutObjectCommand({
    Bucket:      bucketName,
    Key:         key,
    Body:        JSON.stringify(sampleData, null, 2),
    ContentType: 'application/json',
  }));

  logger.info(`Uploaded sample monitoring data to S3: s3://${bucketName}/${key}`);
}

async function sendNotificationEmail(
  sesClient: SESClient,
  senderEmail: string,
  recipientEmail: string,
): Promise<void> {
  // Only tolerate MessageRejected (unverified addresses in sandbox).
  // All other errors — including AccessDeniedException — propagate so the
  // minimizer can detect missing permissions.
  try {
    const subject = 'AWS Monitoring System - Setup Complete';
    const body    = 'Your AWS monitoring system has been successfully set up.\n\nThe system is now ready for use.';

    await sesClient.send(new SendEmailCommand({
      Source:      senderEmail,
      Destination: { ToAddresses: [recipientEmail] },
      Message: {
        Subject: { Data: subject },
        Body:    { Text: { Data: body } },
      },
    }));

    logger.info(`Sent notification email to ${recipientEmail}`);
  } catch (err: unknown) {
    if (err instanceof Error && err.name === 'MessageRejected') {
      logger.warn('SES SendEmail: email not verified (non-fatal)');
    } else {
      throw err; // Re-throw AccessDeniedException and other errors
    }
  }
}

// ── Main logic ────────────────────────────────────────────────────────────────

async function run(cfg: RunConfig): Promise<void> {
  const { bucketName, region } = cfg;

  const s3Client  = new S3Client({ region });
  const sesClient = new SESClient({ region });
  const stsClient = new STSClient({ region });

  // 1. STS GetCallerIdentity — verify credentials
  const identity = await stsClient.send(new GetCallerIdentityCommand({}));
  logger.info(`AWS Account ID: ${identity.Account}`);

  // 2. S3 PutObject — upload monitoring data
  await uploadSampleData(s3Client, bucketName);

  // 3. SES SendEmail — MessageRejected is tolerated; permission errors propagate
  await sendNotificationEmail(sesClient, 'test@example.com', 'test@example.com');

  logger.info('='.repeat(60));
  logger.info('APPLICATION COMPLETED SUCCESSFULLY!');
  logger.info('='.repeat(60));
  logger.info('Resources used:');
  logger.info(`  - S3 Bucket: ${bucketName}`);
  logger.info('='.repeat(60));
  logger.info('To destroy infrastructure, run: cd ../cdk && npx cdk destroy');
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

  logger.info('Starting AWS Comprehensive Monitoring System (data-plane)...');
  logger.info(`Using bucket: ${cfg.bucketName}`);
  logger.info(`Using region: ${cfg.region}`);

  try {
    await run(cfg);
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
