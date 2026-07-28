#!/usr/bin/env node
import * as cdk from 'aws-cdk-lib';
import { AuditLogArchiverStack } from '../lib/stack';

const app = new cdk.App();
new AuditLogArchiverStack(app, 'AuditLogArchiverStack', {
  env: { account: process.env.CDK_DEFAULT_ACCOUNT, region: process.env.CDK_DEFAULT_REGION ?? 'us-east-1' },
});
