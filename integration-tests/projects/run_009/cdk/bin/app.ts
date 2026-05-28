#!/usr/bin/env node
import 'source-map-support/register';
import * as cdk from 'aws-cdk-lib';
import { ComplianceMonitoringStack } from '../lib/stack';

const app = new cdk.App();
new ComplianceMonitoringStack(app, 'ComplianceMonitoringStack-run005-98c7d54c', {
  env: {
    account: app.node.tryGetContext('account') ?? process.env.CDK_DEFAULT_ACCOUNT,
    region:  app.node.tryGetContext('region')  ?? process.env.CDK_DEFAULT_REGION ?? 'us-east-1',
  },
});
