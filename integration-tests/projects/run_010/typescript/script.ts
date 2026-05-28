#!/usr/bin/env node

import * as fs from 'fs';
import * as path from 'path';
import {
  LambdaClient,
  InvokeCommand,
  InvocationType,
} from '@aws-sdk/client-lambda';
import {
  STSClient,
  GetCallerIdentityCommand,
} from '@aws-sdk/client-sts';

// ── Config loading ─────────────────────────────────────────────────────────────

interface RunConfig {
  functionName: string;
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
}

const logger = new Logger();

// ── Main logic ────────────────────────────────────────────────────────────────

async function runDemo(cfg: RunConfig): Promise<void> {
  const { functionName, logGroupName, region } = cfg;

  const stsClient    = new STSClient({ region });
  const lambdaClient = new LambdaClient({ region });

  // 1. STS GetCallerIdentity
  logger.info('Getting AWS account information...');
  const identity = await stsClient.send(new GetCallerIdentityCommand({}));
  logger.info(`Running as: ${identity.Arn}`);

  // 2. Lambda InvokeFunction
  logger.info(`Invoking Lambda function: ${functionName}`);
  const invokeResponse = await lambdaClient.send(new InvokeCommand({
    FunctionName:   functionName,
    InvocationType: InvocationType.RequestResponse,
  }));

  const statusCode = invokeResponse.StatusCode;
  const payload = invokeResponse.Payload
    ? Buffer.from(invokeResponse.Payload).toString('utf-8')
    : '';

  logger.info(`Lambda invocation status: ${statusCode}`);
  logger.info(`Lambda response: ${payload}`);

  if (statusCode !== 200) {
    throw new Error(`Lambda invocation returned unexpected status: ${statusCode}`);
  }

  logger.info('='.repeat(60));
  logger.info('APPLICATION COMPLETED SUCCESSFULLY!');
  logger.info('='.repeat(60));
  logger.info('Resources used:');
  logger.info(`  - Lambda Function: ${functionName}`);
  logger.info(`  - Log Group:       ${logGroupName}`);
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

  logger.info('Starting AWS Deployment Monitoring...');
  logger.info(`Using function:  ${cfg.functionName}`);
  logger.info(`Using log group: ${cfg.logGroupName}`);
  logger.info(`Using region:    ${cfg.region}`);

  try {
    await runDemo(cfg);
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
