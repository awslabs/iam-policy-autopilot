#!/usr/bin/env node
/**
 * CDK App entry point for run_004: ML Monitoring Platform
 *
 * Usage:
 *   # Install dependencies (first time)
 *   npm install
 *
 *   # Bootstrap (first time per account/region)
 *   npx cdk bootstrap
 *
 *   # Deploy infrastructure and write config.json
 *   bash deploy.sh
 *
 *   # Run the data-plane script with the CDK outputs
 *   python ../python/script.py
 *   go run ../go/script.go
 *   cd ../java && mvn exec:java -Dexec.mainClass=Script
 *   cd ../typescript && npm install && npx ts-node script.ts
 *
 *   # Destroy infrastructure when done
 *   npx cdk destroy
 */

import 'source-map-support/register';
import * as cdk from 'aws-cdk-lib';
import { MLMonitoringStack } from '../lib/stack';

const app = new cdk.App();

new MLMonitoringStack(app, 'MLMonitoringStack-run002-b82fea53', {
  // Synthesise for the default account/region from the environment.
  // Override by setting CDK_DEFAULT_ACCOUNT / CDK_DEFAULT_REGION, or by
  // passing --context account=... region=... on the CLI.
  env: {
    account: app.node.tryGetContext('account') ?? process.env.CDK_DEFAULT_ACCOUNT,
    region:  app.node.tryGetContext('region')  ?? process.env.CDK_DEFAULT_REGION ?? 'us-east-1',
  },
});
