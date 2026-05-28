/**
 * CDK Stack for run_005: Secure Repository Monitoring
 *
 * Provisions the control-plane infrastructure:
 *   - KMS key + alias    for encryption (alias/repo-monitor-run003-3cbeaff7)
 *   - SNS topic          repo-alerts-run003-3cbeaff7 (KMS-encrypted, email subscription)
 *   - CodeCommit repo    secure-repo-run003-3cbeaff7
 *   - Secrets Manager    repo-config-run003-3cbeaff7 (initial JSON config, KMS-encrypted)
 *
 * Outputs (consumed by the refactored scripts via config.json):
 *   TopicArn    → SNS topic ARN
 *   SecretName  → Secrets Manager secret name
 *   SecretArn   → Secrets Manager secret ARN
 *   KmsKeyId    → KMS key ID
 *   KmsKeyArn   → KMS key ARN
 *   RepoName    → CodeCommit repository name
 *   CloneUrl    → CodeCommit HTTPS clone URL
 *   Region      → deployment region
 */

import * as cdk from 'aws-cdk-lib';
import * as kms from 'aws-cdk-lib/aws-kms';
import * as sns from 'aws-cdk-lib/aws-sns';
import * as snsSubscriptions from 'aws-cdk-lib/aws-sns-subscriptions';
import * as codecommit from 'aws-cdk-lib/aws-codecommit';
import * as secretsmanager from 'aws-cdk-lib/aws-secretsmanager';
import { Construct } from 'constructs';

export class SecureRepoMonitoringStack extends cdk.Stack {
  public readonly kmsKey: kms.Key;
  public readonly topic: sns.Topic;
  public readonly repository: codecommit.Repository;
  public readonly secret: secretsmanager.Secret;

  constructor(scope: Construct, id: string, props?: cdk.StackProps) {
    super(scope, id, props);

    // ── KMS Key ───────────────────────────────────────────────────────────────
    // Customer-managed key for SNS and Secrets Manager encryption.
    this.kmsKey = new kms.Key(this, 'RepoMonitorKey', {
      description: 'Key for secure repo monitoring system (run_005)',
      removalPolicy: cdk.RemovalPolicy.DESTROY,
      pendingWindow: cdk.Duration.days(7),
      enableKeyRotation: true,
    });

    // ── KMS Alias ─────────────────────────────────────────────────────────────
    new kms.Alias(this, 'RepoMonitorKeyAlias', {
      aliasName: 'alias/repo-monitor-run003-3cbeaff7',
      targetKey: this.kmsKey,
      removalPolicy: cdk.RemovalPolicy.DESTROY,
    });

    // ── SNS Topic ─────────────────────────────────────────────────────────────
    // Encrypted topic for repository monitoring alerts.
    this.topic = new sns.Topic(this, 'RepoAlertsTopic', {
      topicName: 'repo-alerts-run003-3cbeaff7',
      masterKey: this.kmsKey,
    });
    (this.topic.node.defaultChild as cdk.CfnResource).applyRemovalPolicy(
      cdk.RemovalPolicy.DESTROY,
    );

    // Email subscription (requires manual confirmation after deploy).
    this.topic.addSubscription(
      new snsSubscriptions.EmailSubscription('admin@example.com'),
    );

    // ── CodeCommit Repository ─────────────────────────────────────────────────
    // Secure repository for the monitoring system.
    this.repository = new codecommit.Repository(this, 'SecureRepo', {
      repositoryName: 'secure-repo-run003-3cbeaff7',
      description: 'Secure repository for monitoring system',
    });
    this.repository.applyRemovalPolicy(cdk.RemovalPolicy.DESTROY);

    // ── Secrets Manager Secret ────────────────────────────────────────────────
    // Stores the initial monitoring configuration, encrypted with the KMS key.
    const initialSecretValue = JSON.stringify({
      repository_name: 'secure-repo-run003-3cbeaff7',
      sns_topic_arn: this.topic.topicArn,
      kms_key_id: this.kmsKey.keyId,
      monitoring_enabled: true,
      security_features: [
        'KMS encryption',
        'SNS notifications',
        'Secrets Manager integration',
      ],
    });

    this.secret = new secretsmanager.Secret(this, 'RepoConfigSecret', {
      secretName: 'repo-config-run003-3cbeaff7',
      description: 'Configuration for secure repository monitoring system',
      encryptionKey: this.kmsKey,
      secretStringValue: cdk.SecretValue.unsafePlainText(initialSecretValue),
      removalPolicy: cdk.RemovalPolicy.DESTROY,
    });

    // ── Outputs ───────────────────────────────────────────────────────────────
    new cdk.CfnOutput(this, 'TopicArn', {
      value: this.topic.topicArn,
      description: 'SNS topic ARN for repository alerts',
      exportName: `${id}-TopicArn`,
    });

    new cdk.CfnOutput(this, 'SecretName', {
      value: 'repo-config-run003-3cbeaff7',
      description: 'Secrets Manager secret name',
      exportName: `${id}-SecretName`,
    });

    new cdk.CfnOutput(this, 'SecretArn', {
      value: this.secret.secretArn,
      description: 'Secrets Manager secret ARN',
      exportName: `${id}-SecretArn`,
    });

    new cdk.CfnOutput(this, 'KmsKeyId', {
      value: this.kmsKey.keyId,
      description: 'KMS key ID for encryption',
      exportName: `${id}-KmsKeyId`,
    });

    new cdk.CfnOutput(this, 'KmsKeyArn', {
      value: this.kmsKey.keyArn,
      description: 'KMS key ARN for encryption',
      exportName: `${id}-KmsKeyArn`,
    });

    new cdk.CfnOutput(this, 'RepoName', {
      value: this.repository.repositoryName,
      description: 'CodeCommit repository name',
      exportName: `${id}-RepoName`,
    });

    new cdk.CfnOutput(this, 'CloneUrl', {
      value: this.repository.repositoryCloneUrlHttp,
      description: 'CodeCommit HTTPS clone URL',
      exportName: `${id}-CloneUrl`,
    });

    new cdk.CfnOutput(this, 'Region', {
      value: cdk.Stack.of(this).region,
      description: 'Deployment region',
      exportName: `${id}-Region`,
    });
  }
}
