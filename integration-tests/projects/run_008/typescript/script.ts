import {
  CloudWatchLogsClient,
  CreateLogStreamCommand,
  PutLogEventsCommand,
} from '@aws-sdk/client-cloudwatch-logs';
import {
  ServiceCatalogClient,
  ListPortfoliosCommand,
  SearchProductsAsAdminCommand,
} from '@aws-sdk/client-service-catalog';
import {
  STSClient,
  GetCallerIdentityCommand,
} from '@aws-sdk/client-sts';
import * as fs from 'fs';
import * as path from 'path';

// ── Config loading ─────────────────────────────────────────────────────────

interface RunConfig {
  logGroupName: string;
  region: string;
}

function loadConfig(): RunConfig {
  const configPath = path.resolve(__dirname, '..', 'config.json');
  if (!fs.existsSync(configPath)) {
    throw new Error(
      `config.json not found at ${configPath}.\n` +
      'Deploy the CDK stack first:\n' +
      '  cd ../cdk && bash deploy.sh'
    );
  }
  const raw = fs.readFileSync(configPath, 'utf-8');
  return JSON.parse(raw) as RunConfig;
}

// ── CloudWatch helper ──────────────────────────────────────────────────────

async function logToCloudWatch(
  logsClient: CloudWatchLogsClient,
  logGroupName: string,
  logStreamName: string,
  message: string
): Promise<void> {
  // Create log stream (ignore ResourceAlreadyExistsException)
  try {
    await logsClient.send(new CreateLogStreamCommand({
      logGroupName,
      logStreamName,
    }));
  } catch (err: any) {
    if (err.name !== 'ResourceAlreadyExistsException') {
      throw err;  // Re-throw AccessDeniedException and other unexpected errors
    }
  }

  await logsClient.send(new PutLogEventsCommand({
    logGroupName,
    logStreamName,
    logEvents: [
      {
        timestamp: Date.now(),
        message,
      },
    ],
  }));

  console.log(`Logged to CloudWatch: ${message}`);
}

// ── Service Catalog helpers ────────────────────────────────────────────────

interface ProductInfo {
  id: string;
  name: string;
}

interface PortfolioInfo {
  id: string;
  name: string;
  products: ProductInfo[];
}

async function listPortfoliosAndProducts(
  scClient: ServiceCatalogClient,
  logsClient: CloudWatchLogsClient,
  logGroupName: string,
  logStreamName: string
): Promise<PortfolioInfo[]> {
  const portfoliosResponse = await scClient.send(new ListPortfoliosCommand({}));
  const portfolioDetails = portfoliosResponse.PortfolioDetails ?? [];

  const portfolioInfo: PortfolioInfo[] = [];

  for (const portfolio of portfolioDetails) {
    const portfolioId = portfolio.Id!;
    const portfolioName = portfolio.DisplayName!;

    const productList: ProductInfo[] = [];
    try {
      const productsResponse = await scClient.send(new SearchProductsAsAdminCommand({
        PortfolioId: portfolioId,
      }));
      for (const product of productsResponse.ProductViewDetails ?? []) {
        productList.push({
          id: product.ProductViewSummary!.ProductId!,
          name: product.ProductViewSummary!.Name!,
        });
      }
    } catch (err: any) {
      console.warn(`Failed to get products for portfolio ${portfolioId}: ${err.message}`);
    }

    portfolioInfo.push({
      id: portfolioId,
      name: portfolioName,
      products: productList,
    });
  }

  const infoMsg = `Found ${portfolioInfo.length} portfolios`;
  console.log(infoMsg);
  await logToCloudWatch(logsClient, logGroupName, logStreamName, infoMsg);

  for (const portfolio of portfolioInfo) {
    const detailMsg = `Portfolio: ${portfolio.name} (${portfolio.id}) has ${portfolio.products.length} products`;
    console.log(detailMsg);
    await logToCloudWatch(logsClient, logGroupName, logStreamName, detailMsg);
  }

  return portfolioInfo;
}

// ── Main ───────────────────────────────────────────────────────────────────

async function main(): Promise<void> {
  const cfg = loadConfig();
  const region = cfg.region ?? 'us-east-1';

  console.log('Starting AWS Service Catalog Manager');
  console.log(`Log group: ${cfg.logGroupName}`);
  console.log(`Region:    ${region}`);

  const logsClient = new CloudWatchLogsClient({ region });
  const scClient   = new ServiceCatalogClient({ region });
  const stsClient  = new STSClient({ region });

  // Verify credentials
  const identity = await stsClient.send(new GetCallerIdentityCommand({}));
  console.log(`AWS Account: ${identity.Account}`);

  const logStreamName = `service-catalog-manager-${Date.now()}`;

  // Log startup
  await logToCloudWatch(logsClient, cfg.logGroupName, logStreamName,
    'Service Catalog Manager started');

  // List portfolios and products
  console.log('Listing portfolios and products...');
  const portfolioInfo = await listPortfoliosAndProducts(
    scClient, logsClient, cfg.logGroupName, logStreamName
  );

  // Log completion
  await logToCloudWatch(logsClient, cfg.logGroupName, logStreamName,
    'Service Catalog Manager completed successfully');

  console.log('='.repeat(60));
  console.log('SERVICE CATALOG MANAGER COMPLETED');
  console.log('='.repeat(60));
  console.log(`Region:           ${region}`);
  console.log(`Log Group:        ${cfg.logGroupName}`);
  console.log(`Log Stream:       ${logStreamName}`);
  console.log(`Portfolios found: ${portfolioInfo.length}`);
  console.log('='.repeat(60));
}

main().catch((err) => {
  console.error('Service Catalog Manager failed:', err);
  process.exit(1);
});
