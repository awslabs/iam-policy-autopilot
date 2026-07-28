#!/usr/bin/env node
import * as cdk from 'aws-cdk-lib';
import { DocumentPipelineStack } from '../lib/stack';

const app = new cdk.App();
new DocumentPipelineStack(app, 'DocumentPipelineStack', {
  env: {
    account: process.env.CDK_DEFAULT_ACCOUNT,
    region: process.env.CDK_DEFAULT_REGION ?? 'us-east-1',
  },
});
