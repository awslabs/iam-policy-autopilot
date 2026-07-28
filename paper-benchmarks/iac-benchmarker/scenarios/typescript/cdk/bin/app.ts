#!/usr/bin/env node
import * as cdk from 'aws-cdk-lib';
import { FeatureFlagServiceStack } from '../lib/stack';

const app = new cdk.App();
new FeatureFlagServiceStack(app, 'FeatureFlagServiceStack', {
  env: { account: process.env.CDK_DEFAULT_ACCOUNT, region: process.env.CDK_DEFAULT_REGION ?? 'us-east-1' },
});
