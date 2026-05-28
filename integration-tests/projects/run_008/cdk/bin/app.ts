#!/usr/bin/env node
import 'source-map-support/register';
import * as cdk from 'aws-cdk-lib';
import { ServiceCatalogManagerStack } from '../lib/stack';

const app = new cdk.App();
new ServiceCatalogManagerStack(app, 'ServiceCatalogManagerStack-run004-a6a046d0', {
  env: {
    account: app.node.tryGetContext('account') ?? process.env.CDK_DEFAULT_ACCOUNT,
    region:  app.node.tryGetContext('region')  ?? process.env.CDK_DEFAULT_REGION ?? 'us-east-1',
  },
});
