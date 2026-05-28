import software.amazon.awssdk.regions.Region;
import software.amazon.awssdk.services.cloudwatchlogs.CloudWatchLogsClient;
import software.amazon.awssdk.services.cloudwatchlogs.model.*;
import software.amazon.awssdk.services.servicecatalog.ServiceCatalogClient;
import software.amazon.awssdk.services.servicecatalog.model.*;
import software.amazon.awssdk.services.sts.StsClient;
import software.amazon.awssdk.services.sts.model.*;

import com.fasterxml.jackson.databind.ObjectMapper;

import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

import java.io.File;
import java.util.*;

public class Script {
    private static final Logger logger = LoggerFactory.getLogger(Script.class);
    private static final ObjectMapper objectMapper = new ObjectMapper();

    // ── Config loading ─────────────────────────────────────────────────────────

    static class RunConfig {
        public String logGroupName;
        public String region;
    }

    private static RunConfig loadConfig() throws Exception {
        // config.json lives one directory above the java/ directory (at the run level).
        File configFile = new File(System.getProperty("user.dir"), "../config.json")
                .getCanonicalFile();

        if (!configFile.exists()) {
            throw new RuntimeException(
                "config.json not found at " + configFile.getAbsolutePath() + ".\n" +
                "Deploy the CDK stack first:\n" +
                "  cd ../cdk && bash deploy.sh"
            );
        }
        return objectMapper.readValue(configFile, RunConfig.class);
    }

    // ── Entry point ────────────────────────────────────────────────────────────

    public static void main(String[] args) {
        try {
            RunConfig cfg = loadConfig();
            Region region = Region.of(cfg.region != null ? cfg.region : "us-east-1");

            logger.info("Starting AWS Service Catalog Manager...");
            logger.info("Using log group: {}", cfg.logGroupName);
            logger.info("Using region:    {}", region);

            runDemo(cfg, region);

        } catch (Exception e) {
            logger.error("Application failed: {}", e.getMessage());
            System.exit(1);
        }
    }

    // ── Data-plane helpers ─────────────────────────────────────────────────────

    /**
     * Create a log stream (if needed) and put a log event to CloudWatch.
     */
    private static void logToCloudWatch(
            CloudWatchLogsClient logsClient,
            String logGroupName,
            String logStreamName,
            String message) {
        // CreateLogStream (ignore only ResourceAlreadyExistsException)
        try {
            logsClient.createLogStream(CreateLogStreamRequest.builder()
                    .logGroupName(logGroupName)
                    .logStreamName(logStreamName)
                    .build());
        } catch (ResourceAlreadyExistsException e) {
            // Stream already exists — continue
        }

        InputLogEvent logEvent = InputLogEvent.builder()
                .timestamp(System.currentTimeMillis())
                .message(message)
                .build();

        // No catch — let AccessDeniedException propagate so the minimizer
        // detects that logs:PutLogEvents is required.
        logsClient.putLogEvents(PutLogEventsRequest.builder()
                .logGroupName(logGroupName)
                .logStreamName(logStreamName)
                .logEvents(logEvent)
                .build());

        logger.info("Logged to CloudWatch: {}", message);
    }

    /**
     * List all portfolios and search products within each portfolio.
     */
    private static List<Map<String, Object>> listPortfoliosAndProducts(
            ServiceCatalogClient scClient,
            CloudWatchLogsClient logsClient,
            String logGroupName,
            String logStreamName) {

        ListPortfoliosResponse portfoliosResponse = scClient.listPortfolios(
                ListPortfoliosRequest.builder().build());

        List<Map<String, Object>> portfolioInfo = new ArrayList<>();

        for (PortfolioDetail portfolio : portfoliosResponse.portfolioDetails()) {
            String portfolioId = portfolio.id();
            String portfolioName = portfolio.displayName();

            List<Map<String, String>> productList = new ArrayList<>();
            try {
                SearchProductsAsAdminResponse productsResponse = scClient.searchProductsAsAdmin(
                        SearchProductsAsAdminRequest.builder()
                                .portfolioId(portfolioId)
                                .build());

                for (ProductViewDetail product : productsResponse.productViewDetails()) {
                    Map<String, String> productMap = new HashMap<>();
                    productMap.put("Id", product.productViewSummary().productId());
                    productMap.put("Name", product.productViewSummary().name());
                    productList.add(productMap);
                }
            } catch (Exception e) {
                logger.warn("Failed to get products for portfolio {}: {}", portfolioId, e.getMessage());
            }

            Map<String, Object> portfolioMap = new HashMap<>();
            portfolioMap.put("Id", portfolioId);
            portfolioMap.put("Name", portfolioName);
            portfolioMap.put("Products", productList);
            portfolioInfo.add(portfolioMap);
        }

        String infoMsg = "Found " + portfolioInfo.size() + " portfolios";
        logger.info(infoMsg);
        logToCloudWatch(logsClient, logGroupName, logStreamName, infoMsg);

        for (Map<String, Object> portfolio : portfolioInfo) {
            @SuppressWarnings("unchecked")
            List<Map<String, String>> products = (List<Map<String, String>>) portfolio.get("Products");
            String detailMsg = "Portfolio: " + portfolio.get("Name") +
                    " (" + portfolio.get("Id") + ") has " + products.size() + " products";
            logger.info(detailMsg);
            logToCloudWatch(logsClient, logGroupName, logStreamName, detailMsg);
        }

        return portfolioInfo;
    }

    // ── Main logic ────────────────────────────────────────────────────────────

    private static void runDemo(RunConfig cfg, Region region) throws Exception {
        CloudWatchLogsClient logsClient = CloudWatchLogsClient.builder().region(region).build();
        ServiceCatalogClient scClient   = ServiceCatalogClient.builder().region(region).build();
        StsClient stsClient             = StsClient.builder().region(region).build();

        try {
            // 1. STS GetCallerIdentity — verify credentials
            logger.info("Verifying AWS credentials...");
            GetCallerIdentityResponse identity = stsClient.getCallerIdentity();
            logger.info("AWS Account ID: {}", identity.account());

            String logStreamName = "service-catalog-manager-" + System.currentTimeMillis();

            // 2. Log startup
            logToCloudWatch(logsClient, cfg.logGroupName, logStreamName,
                    "Service Catalog Manager started");

            // 3. List portfolios and products
            logger.info("Listing portfolios and products...");
            List<Map<String, Object>> portfolioInfo = listPortfoliosAndProducts(
                    scClient, logsClient, cfg.logGroupName, logStreamName);

            // 4. Log completion
            String completionMsg = "Service Catalog Manager completed successfully";
            logToCloudWatch(logsClient, cfg.logGroupName, logStreamName, completionMsg);

            logger.info("============================================================");
            logger.info("SERVICE CATALOG MANAGER COMPLETED");
            logger.info("============================================================");
            logger.info("Region:           {}", region);
            logger.info("Log Group:        {}", cfg.logGroupName);
            logger.info("Log Stream:       {}", logStreamName);
            logger.info("Portfolios found: {}", portfolioInfo.size());
            logger.info("============================================================");
            logger.info("To destroy infrastructure, run: cd ../cdk && npx cdk destroy");

        } finally {
            logsClient.close();
            scClient.close();
            stsClient.close();
        }
    }
}
