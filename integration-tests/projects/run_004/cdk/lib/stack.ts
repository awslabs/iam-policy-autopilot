/**
 * CDK Stack for run_004: ML Monitoring Platform
 *
 * Provisions the control-plane infrastructure:
 *   - KMS key + alias       for encryption (alias/ml-monitoring-run002-b82fea53)
 *   - CloudWatch log group  /aws/ecs/ml-monitoring-run002-b82fea53 (KMS-encrypted)
 *   - IAM role              for ECS task execution (AmazonECSTaskExecutionRolePolicy)
 *   - ECS Cluster           ml-cluster-run002-b82fea53 (tag: Project=MLMonitoring)
 *   - ECS Task Definition   ml-cluster-run002-b82fea53-task (FARGATE, nginx:latest)
 *   - Resource Group        ml-resources-run002-b82fea53 (tag-based: Project=MLMonitoring)
 *   - X-Ray Sampling Rule   MLMonitoringSampling (priority 9000, fixedRate 0.1)
 *
 * Outputs (consumed by the refactored scripts via config.json):
 *   ClusterName        → ECS cluster name
 *   ClusterArn         → ECS cluster ARN
 *   LogGroupName       → CloudWatch log group name
 *   KmsKeyId           → KMS key ID
 *   KmsKeyArn          → KMS key ARN
 *   ResourceGroupName  → resource group name
 *   Region             → deployment region
 */

import * as cdk from 'aws-cdk-lib';
import * as kms from 'aws-cdk-lib/aws-kms';
import * as logs from 'aws-cdk-lib/aws-logs';
import * as iam from 'aws-cdk-lib/aws-iam';
import * as ecs from 'aws-cdk-lib/aws-ecs';
import * as resourcegroups from 'aws-cdk-lib/aws-resourcegroups';
import * as xray from 'aws-cdk-lib/aws-xray';
import { Construct } from 'constructs';

export class MLMonitoringStack extends cdk.Stack {
  public readonly kmsKey: kms.Key;
  public readonly logGroup: logs.LogGroup;
  public readonly cluster: ecs.Cluster;

  constructor(scope: Construct, id: string, props?: cdk.StackProps) {
    super(scope, id, props);

    // ── KMS Key ───────────────────────────────────────────────────────────────
    // Customer-managed key for ML Monitoring Platform encryption.
    this.kmsKey = new kms.Key(this, 'MLMonitoringKey', {
      description: 'ML Monitoring Platform encryption key (run_004)',
      removalPolicy: cdk.RemovalPolicy.DESTROY,
      pendingWindow: cdk.Duration.days(7),
      enableKeyRotation: true,
    });

    // CloudWatch Logs requires an explicit key policy grant to use a CMK.
    // Without this, CloudFormation fails when creating the encrypted log group.
    // See: https://docs.aws.amazon.com/AmazonCloudWatch/latest/logs/encrypt-log-data-kms.html
    this.kmsKey.addToResourcePolicy(new iam.PolicyStatement({
      sid: 'AllowCloudWatchLogsEncryption',
      effect: iam.Effect.ALLOW,
      principals: [new iam.ServicePrincipal(`logs.${cdk.Stack.of(this).region}.amazonaws.com`)],
      actions: [
        'kms:Encrypt*',
        'kms:Decrypt*',
        'kms:ReEncrypt*',
        'kms:GenerateDataKey*',
        'kms:Describe*',
      ],
      resources: ['*'],
      conditions: {
        ArnLike: {
          'kms:EncryptionContext:aws:logs:arn': `arn:aws:logs:${cdk.Stack.of(this).region}:${cdk.Stack.of(this).account}:log-group:/aws/ecs/ml-monitoring-run002-b82fea53`,
        },
      },
    }));

    // ── KMS Alias ─────────────────────────────────────────────────────────────
    new kms.Alias(this, 'MLMonitoringKeyAlias', {
      aliasName: 'alias/ml-monitoring-run002-b82fea53',
      targetKey: this.kmsKey,
      removalPolicy: cdk.RemovalPolicy.DESTROY,
    });

    // ── CloudWatch Log Group ──────────────────────────────────────────────────
    // Receives logs from the ECS task via the awslogs log driver.
    this.logGroup = new logs.LogGroup(this, 'MLMonitoringLogGroup', {
      logGroupName: '/aws/ecs/ml-monitoring-run002-b82fea53',
      encryptionKey: this.kmsKey,
      removalPolicy: cdk.RemovalPolicy.DESTROY,
      retention: logs.RetentionDays.ONE_WEEK,
    });

    // ── IAM Role for ECS Task Execution ───────────────────────────────────────
    // Allows ECS to pull container images and write logs to CloudWatch.
    const ecsTaskExecutionRole = new iam.Role(this, 'ECSTaskExecutionRole', {
      assumedBy: new iam.ServicePrincipal('ecs-tasks.amazonaws.com'),
      description: 'ECS task execution role for ML Monitoring Platform',
      managedPolicies: [
        iam.ManagedPolicy.fromAwsManagedPolicyName('service-role/AmazonECSTaskExecutionRolePolicy'),
      ],
    });

    // ── ECS Cluster ───────────────────────────────────────────────────────────
    // Hosts the ML monitoring Fargate tasks.
    this.cluster = new ecs.Cluster(this, 'MLMonitoringCluster', {
      clusterName: 'ml-cluster-run002-b82fea53',
    });

    // Apply tags and removal policy via the underlying CfnCluster.
    const cfnCluster = this.cluster.node.defaultChild as ecs.CfnCluster;
    cfnCluster.applyRemovalPolicy(cdk.RemovalPolicy.DESTROY);
    cdk.Tags.of(this.cluster).add('Project', 'MLMonitoring');

    // ── ECS Task Definition ───────────────────────────────────────────────────
    // FARGATE task running nginx:latest with awslogs log driver.
    const taskDefinition = new ecs.FargateTaskDefinition(this, 'MLMonitoringTaskDef', {
      family: 'ml-cluster-run002-b82fea53-task',
      cpu: 256,
      memoryLimitMiB: 512,
      executionRole: ecsTaskExecutionRole,
    });

    taskDefinition.addContainer('ml-monitor', {
      image: ecs.ContainerImage.fromRegistry('nginx:latest'),
      logging: ecs.LogDrivers.awsLogs({
        streamPrefix: 'ml-monitor',
        logGroup: this.logGroup,
      }),
    });

    // ── Resource Group ────────────────────────────────────────────────────────
    // Tag-based group collecting all resources tagged Project=MLMonitoring.
    new resourcegroups.CfnGroup(this, 'MLMonitoringResourceGroup', {
      name: 'ml-resources-run002-b82fea53',
      resourceQuery: {
        type: 'TAG_FILTERS_1_0',
        query: {
          resourceTypeFilters: ['AWS::AllSupported'],
          tagFilters: [
            {
              key: 'Project',
              values: ['MLMonitoring'],
            },
          ],
        },
      },
    }).applyRemovalPolicy(cdk.RemovalPolicy.DESTROY);

    // ── X-Ray Sampling Rule ───────────────────────────────────────────────────
    // Samples 10% of requests to the ml-monitoring service.
    new xray.CfnSamplingRule(this, 'MLMonitoringSamplingRule', {
      samplingRule: {
        ruleName: 'MLMonitoringSampling',
        priority: 9000,
        fixedRate: 0.1,
        reservoirSize: 1,
        serviceName: 'ml-monitoring',
        serviceType: '*',
        host: '*',
        httpMethod: '*',
        urlPath: '*',
        resourceArn: '*',
        version: 1,
      },
    }).applyRemovalPolicy(cdk.RemovalPolicy.DESTROY);

    // ── Outputs ───────────────────────────────────────────────────────────────
    new cdk.CfnOutput(this, 'ClusterName', {
      value: this.cluster.clusterName,
      description: 'ECS cluster name',
      exportName: `${id}-ClusterName`,
    });

    new cdk.CfnOutput(this, 'ClusterArn', {
      value: this.cluster.clusterArn,
      description: 'ECS cluster ARN',
      exportName: `${id}-ClusterArn`,
    });

    new cdk.CfnOutput(this, 'LogGroupName', {
      value: this.logGroup.logGroupName,
      description: 'CloudWatch log group name for ECS tasks',
      exportName: `${id}-LogGroupName`,
    });

    new cdk.CfnOutput(this, 'KmsKeyId', {
      value: this.kmsKey.keyId,
      description: 'KMS key ID for ML Monitoring Platform encryption',
      exportName: `${id}-KmsKeyId`,
    });

    new cdk.CfnOutput(this, 'KmsKeyArn', {
      value: this.kmsKey.keyArn,
      description: 'KMS key ARN for ML Monitoring Platform encryption',
      exportName: `${id}-KmsKeyArn`,
    });

    new cdk.CfnOutput(this, 'ResourceGroupName', {
      value: 'ml-resources-run002-b82fea53',
      description: 'Resource group name for ML Monitoring Platform resources',
      exportName: `${id}-ResourceGroupName`,
    });

    new cdk.CfnOutput(this, 'Region', {
      value: cdk.Stack.of(this).region,
      description: 'Deployment region',
      exportName: `${id}-Region`,
    });
  }
}
