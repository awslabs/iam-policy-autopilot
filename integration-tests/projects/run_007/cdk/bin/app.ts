#!/usr/bin/env node
import 'source-map-support/register';
import * as cdk from 'aws-cdk-lib';
import { ComprehensiveMonitoringStack } from '../lib/stack';

const app = new cdk.App();
new ComprehensiveMonitoringStack(app, 'ComprehensiveMonitoringStack-run004-897f3738', {
  env: {
    account: app.node.tryGetContext('account') ?? process.env.CDK_DEFAULT_ACCOUNT,
    region:  app.node.tryGetContext('region')  ?? process.env.CDK_DEFAULT_REGION ?? 'us-east-1',
  },
});
