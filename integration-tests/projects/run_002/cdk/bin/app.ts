#!/usr/bin/env node
/**
 * CDK App entry point for run_002: File Processing Monitoring System
 *
 * Usage:
 *   # Install dependencies (first time)
 *   npm install
 *
 *   # Bootstrap (first time per account/region)
 *   npx cdk bootstrap
 *
 *   # Deploy infrastructure
 *   npx cdk deploy
 *
 *   # Run the data-plane script with the CDK outputs
 *   python script.py \
 *     --bucket  <BucketName from CDK output> \
 *     --queue-url <QueueUrl from CDK output> \
 *     --region  us-east-1
 *
 *   # Destroy infrastructure when done
 *   npx cdk destroy
 */

import 'source-map-support/register';
import * as cdk from 'aws-cdk-lib';
import { FileMonitoringStack } from '../lib/stack';

const app = new cdk.App();

new FileMonitoringStack(app, 'FileMonitoringStack-run001', {
  // Synthesise for the default account/region from the environment.
  // Override by setting CDK_DEFAULT_ACCOUNT / CDK_DEFAULT_REGION, or by
  // passing --context account=... region=... on the CLI.
  env: {
    account: app.node.tryGetContext('account') ?? process.env.CDK_DEFAULT_ACCOUNT,
    region:  app.node.tryGetContext('region')  ?? process.env.CDK_DEFAULT_REGION ?? 'us-east-1',
  },
});
