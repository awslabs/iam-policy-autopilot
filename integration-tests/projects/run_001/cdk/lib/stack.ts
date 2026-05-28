/**
 * CDK Stack for run_001: AWS Security and Analytics Platform
 *
 * Provisions the control-plane infrastructure that was previously created
 * inline by the original script:
 *   - S3 bucket         for CloudTrail logs (with bucket policy)
 *   - IAM role + policy for EC2/Redshift
 *   - CloudTrail trail  (multi-region, log file validation)
 *   - EC2 instance      (security monitoring, with security group)
 *   - Redshift cluster  (single-node, ra3.xlplus)
 *
 * Outputs (consumed by the refactored scripts via config.json):
 *   BucketName                 → S3 bucket name
 *   RedshiftClusterIdentifier  → Redshift cluster identifier
 *   Region                     → deployment region
 */

import * as cdk from 'aws-cdk-lib';
import * as s3 from 'aws-cdk-lib/aws-s3';
import * as iam from 'aws-cdk-lib/aws-iam';
import * as cloudtrail from 'aws-cdk-lib/aws-cloudtrail';
import * as ec2 from 'aws-cdk-lib/aws-ec2';
import * as redshift from 'aws-cdk-lib/aws-redshift';
import * as secretsmanager from 'aws-cdk-lib/aws-secretsmanager';
import { Construct } from 'constructs';

export class SecurityAnalyticsStack extends cdk.Stack {
  public readonly bucket: s3.Bucket;
  public readonly redshiftCluster: redshift.CfnCluster;

  constructor(scope: Construct, id: string, props?: cdk.StackProps) {
    super(scope, id, props);

    // ── S3 Bucket (CloudTrail logs) ───────────────────────────────────────────
    // Mirrors the original create_s3_bucket() call.
    // DESTROY + autoDeleteObjects replaces the cleanup_resources() logic.
    this.bucket = new s3.Bucket(this, 'SecurityLogsBucket', {
      removalPolicy: cdk.RemovalPolicy.DESTROY,
      autoDeleteObjects: true,
      blockPublicAccess: s3.BlockPublicAccess.BLOCK_ALL,
      encryption: s3.BucketEncryption.S3_MANAGED,
      enforceSSL: true,
    });

    // Bucket policy allowing CloudTrail to write logs
    this.bucket.addToResourcePolicy(
      new iam.PolicyStatement({
        sid: 'AWSCloudTrailAclCheck',
        effect: iam.Effect.ALLOW,
        principals: [new iam.ServicePrincipal('cloudtrail.amazonaws.com')],
        actions: ['s3:GetBucketAcl'],
        resources: [this.bucket.bucketArn],
      }),
    );
    this.bucket.addToResourcePolicy(
      new iam.PolicyStatement({
        sid: 'AWSCloudTrailWrite',
        effect: iam.Effect.ALLOW,
        principals: [new iam.ServicePrincipal('cloudtrail.amazonaws.com')],
        actions: ['s3:PutObject'],
        resources: [`${this.bucket.bucketArn}/AWSLogs/*`],
        conditions: {
          StringEquals: { 's3:x-amz-acl': 'bucket-owner-full-control' },
        },
      }),
    );

    // ── IAM Role + Policy (EC2 / Redshift) ────────────────────────────────────
    // Mirrors the original create_iam_role() call.
    const securityRole = new iam.Role(this, 'SecurityRole', {
      assumedBy: new iam.CompositePrincipal(
        new iam.ServicePrincipal('ec2.amazonaws.com'),
        new iam.ServicePrincipal('redshift.amazonaws.com'),
      ),
      description: 'IAM role for security monitoring EC2 and Redshift',
    });

    // S3 permissions scoped to the security logs bucket
    securityRole.addToPolicy(
      new iam.PolicyStatement({
        effect: iam.Effect.ALLOW,
        actions: ['s3:ListBucket'],
        resources: [this.bucket.bucketArn],
      }),
    );
    securityRole.addToPolicy(
      new iam.PolicyStatement({
        effect: iam.Effect.ALLOW,
        actions: ['s3:GetObject', 's3:PutObject'],
        resources: [`${this.bucket.bucketArn}/*`],
      }),
    );

    // CloudWatch / Logs permissions (account-scoped)
    securityRole.addToPolicy(
      new iam.PolicyStatement({
        effect: iam.Effect.ALLOW,
        actions: [
          'cloudwatch:PutMetricData',
        ],
        resources: ['*'],  // PutMetricData does not support resource-level permissions
        conditions: {
          StringEquals: { 'cloudwatch:namespace': 'SecurityMonitoring' },
        },
      }),
    );
    securityRole.addToPolicy(
      new iam.PolicyStatement({
        effect: iam.Effect.ALLOW,
        actions: [
          'logs:CreateLogGroup',
          'logs:CreateLogStream',
          'logs:PutLogEvents',
        ],
        resources: [
          `arn:aws:logs:${cdk.Stack.of(this).region}:${cdk.Stack.of(this).account}:log-group:/aws/security-monitoring*`,
        ],
      }),
    );

    // Redshift Data API permissions (scoped to this account/region)
    securityRole.addToPolicy(
      new iam.PolicyStatement({
        effect: iam.Effect.ALLOW,
        actions: [
          'redshift-data:ExecuteStatement',
          'redshift-data:GetStatementResult',
          'redshift-data:DescribeStatement',
        ],
        resources: [
          `arn:aws:redshift:${cdk.Stack.of(this).region}:${cdk.Stack.of(this).account}:cluster:*`,
        ],
      }),
    );

    // ── CloudTrail Trail ──────────────────────────────────────────────────────
    // Mirrors the original create_cloudtrail() call.
    // multi-region + log file validation enabled.
    new cloudtrail.Trail(this, 'SecurityTrail', {
      bucket: this.bucket,
      isMultiRegionTrail: true,
      enableFileValidation: true,
      includeGlobalServiceEvents: true,
      sendToCloudWatchLogs: false,
    });

    // ── VPC (default) for EC2 ─────────────────────────────────────────────────
    const vpc = ec2.Vpc.fromLookup(this, 'DefaultVpc', { isDefault: true });

    // ── Security Group for EC2 ────────────────────────────────────────────────
    const securityGroup = new ec2.SecurityGroup(this, 'SecurityMonitorSG', {
      vpc,
      description: 'Security group for security monitoring EC2 instance',
      allowAllOutbound: true,
    });

    // ── EC2 Instance (security monitoring) ───────────────────────────────────
    // Mirrors the original create_ec2_instance() call.
    // t3.micro, Amazon Linux 2.
    new ec2.Instance(this, 'SecurityMonitorInstance', {
      vpc,
      instanceType: ec2.InstanceType.of(ec2.InstanceClass.T3, ec2.InstanceSize.MICRO),
      machineImage: ec2.MachineImage.latestAmazonLinux2(),
      securityGroup,
      role: securityRole,
    });

    // ── Redshift Secret (generated password) ──────────────────────────────────
    // Generate a random password at deploy time via Secrets Manager.
    const dbSecret = new secretsmanager.Secret(this, 'RedshiftSecret', {
      generateSecretString: {
        secretStringTemplate: JSON.stringify({ username: 'adminuser' }),
        generateStringKey: 'password',
        excludePunctuation: true,  // Redshift has password char restrictions
        passwordLength: 16,
      },
    });

    // ── Redshift Cluster (single-node, ra3.xlplus) ────────────────────────────
    // Mirrors the original create_redshift_cluster() call.
    // Using CfnCluster for full parameter control.
    this.redshiftCluster = new redshift.CfnCluster(this, 'SecurityRedshiftCluster', {
      clusterType: 'single-node',
      dbName: 'securitydb',
      masterUsername: 'adminuser',
      masterUserPassword: dbSecret.secretValueFromJson('password').unsafeUnwrap(),
      nodeType: 'ra3.xlplus',
      iamRoles: [securityRole.roleArn],
      publiclyAccessible: false,
    });

    // ── Outputs ───────────────────────────────────────────────────────────────
    new cdk.CfnOutput(this, 'BucketName', {
      value: this.bucket.bucketName,
      description: 'S3 bucket name for CloudTrail logs',
      exportName: `${id}-BucketName`,
    });

    new cdk.CfnOutput(this, 'RedshiftClusterIdentifier', {
      value: this.redshiftCluster.ref,
      description: 'Redshift cluster identifier',
      exportName: `${id}-RedshiftClusterIdentifier`,
    });

    new cdk.CfnOutput(this, 'Region', {
      value: cdk.Stack.of(this).region,
      description: 'Deployment region',
      exportName: `${id}-Region`,
    });
  }
}
