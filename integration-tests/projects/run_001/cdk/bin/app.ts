#!/usr/bin/env node
/**
 * CDK App entry point for run_001: AWS Security and Analytics Platform
 *
 * Usage:
 *   # Install dependencies (first time)
 *   npm install
 *
 *   # Bootstrap (first time per account/region)
 *   npx cdk bootstrap
 *
 *   # Deploy infrastructure
 *   bash deploy.sh
 *
 *   # Run the data-plane script with the CDK outputs (config.json written by deploy.sh)
 *   python ../python/script.py
 *   go run ../go/script.go
 *   npx ts-node ../typescript/script.ts
 *   cd ../java && mvn exec:java -Dexec.mainClass=Script
 *
 *   # Destroy infrastructure when done
 *   npx cdk destroy
 */

import 'source-map-support/register';
import * as cdk from 'aws-cdk-lib';
import { SecurityAnalyticsStack } from '../lib/stack';

const app = new cdk.App();

new SecurityAnalyticsStack(app, 'SecurityAnalyticsStack-run001-0aa559b7', {
  // Synthesise for the default account/region from the environment.
  // Override by setting CDK_DEFAULT_ACCOUNT / CDK_DEFAULT_REGION, or by
  // passing --context account=... region=... on the CLI.
  env: {
    account: app.node.tryGetContext('account') ?? process.env.CDK_DEFAULT_ACCOUNT,
    region:  app.node.tryGetContext('region')  ?? process.env.CDK_DEFAULT_REGION ?? 'us-east-1',
  },
});
