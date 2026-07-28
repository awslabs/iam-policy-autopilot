#!/usr/bin/env node
import * as cdk from 'aws-cdk-lib';
import { MetricsAggregationStack } from '../lib/stack';

const app = new cdk.App();
new MetricsAggregationStack(app, 'MetricsAggregationStack', {
  env: {
    account: process.env.CDK_DEFAULT_ACCOUNT,
    region: process.env.CDK_DEFAULT_REGION ?? 'us-east-1',
  },
});
