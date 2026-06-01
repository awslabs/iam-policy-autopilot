import * as cdk from 'aws-cdk-lib';
import * as logs from 'aws-cdk-lib/aws-logs';
import * as servicecatalog from 'aws-cdk-lib/aws-servicecatalog';
import { Construct } from 'constructs';

export class ServiceCatalogManagerStack extends cdk.Stack {
  public readonly logGroup: logs.LogGroup;
  public readonly portfolio: servicecatalog.Portfolio;

  constructor(scope: Construct, id: string, props?: cdk.StackProps) {
    super(scope, id, props);

    this.logGroup = new logs.LogGroup(this, 'ServiceCatalogLogGroup', {
      logGroupName: '/aws/service-catalog-manager/run004-a6a046d0',
      removalPolicy: cdk.RemovalPolicy.DESTROY,
      retention: logs.RetentionDays.ONE_DAY,
    });

    // Portfolio ensures ListPortfolios returns ≥1 result so the
    // SearchProductsAsAdmin code path inside the for-loop is exercised
    // during policy minimization.
    this.portfolio = new servicecatalog.Portfolio(this, 'TestPortfolio', {
      displayName: 'run004-a6a046d0-test-portfolio',
      providerName: 'IamPolicyAutopilot Test',
      description: 'Test portfolio for policy minimization',
    });

    new cdk.CfnOutput(this, 'LogGroupName', {
      value: this.logGroup.logGroupName,
      exportName: `${id}-LogGroupName`,
    });

    new cdk.CfnOutput(this, 'PortfolioId', {
      value: this.portfolio.portfolioId,
      exportName: `${id}-PortfolioId`,
    });

    new cdk.CfnOutput(this, 'Region', {
      value: cdk.Stack.of(this).region,
      exportName: `${id}-Region`,
    });
  }
}
