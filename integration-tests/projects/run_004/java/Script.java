import software.amazon.awssdk.regions.Region;
import software.amazon.awssdk.services.sts.StsClient;
import software.amazon.awssdk.services.sts.model.*;
import software.amazon.awssdk.services.xray.XRayClient;
import software.amazon.awssdk.services.xray.model.*;

import com.fasterxml.jackson.databind.ObjectMapper;

import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

import java.io.File;

public class Script {
    private static final Logger logger = LoggerFactory.getLogger(Script.class);
    private static final ObjectMapper objectMapper = new ObjectMapper();

    // ── Config loading ─────────────────────────────────────────────────────────

    static class RunConfig {
        public String clusterName;
        public String clusterArn;
        public String logGroupName;
        public String kmsKeyId;
        public String kmsKeyArn;
        public String resourceGroupName;
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

            logger.info("Starting ML Monitoring Platform...");
            logger.info("Using ECS cluster:    {}", cfg.clusterName);
            logger.info("Using log group:      {}", cfg.logGroupName);
            logger.info("Using KMS key:        {}", cfg.kmsKeyId);
            logger.info("Using resource group: {}", cfg.resourceGroupName);
            logger.info("Using region:         {}", region);

            runMLMonitoring(cfg, region);

        } catch (Exception e) {
            logger.error("Application failed: {}", e.getMessage());
            System.exit(1);
        }
    }

    // ── Data-plane helpers ─────────────────────────────────────────────────────

    private static String getAwsAccountId(StsClient stsClient) {
        GetCallerIdentityResponse response = stsClient.getCallerIdentity();
        return response.account();
    }

    private static void configureXRayEncryption(XRayClient xrayClient) {
        xrayClient.putEncryptionConfig(
                PutEncryptionConfigRequest.builder()
                        .type(EncryptionType.NONE)
                        .build());
        logger.info("X-Ray encryption configured (Type=NONE)");
    }

    // ── Main logic ────────────────────────────────────────────────────────────

    private static void runMLMonitoring(RunConfig cfg, Region region) {
        StsClient  stsClient  = StsClient.builder().region(region).build();
        XRayClient xrayClient = XRayClient.builder().region(region).build();

        try {
            // 1. Get account ID
            logger.info("Getting AWS account information...");
            String accountId = getAwsAccountId(stsClient);
            logger.info("AWS Account ID: {}", accountId);

            // 2. Configure X-Ray encryption (data-plane runtime config)
            logger.info("Configuring X-Ray encryption...");
            configureXRayEncryption(xrayClient);

            logger.info("============================================================");
            logger.info("APPLICATION COMPLETED SUCCESSFULLY!");
            logger.info("============================================================");
            logger.info("Resources used:");
            logger.info("  - ECS Cluster:    {}", cfg.clusterName);
            logger.info("  - Log Group:      {}", cfg.logGroupName);
            logger.info("  - KMS Key:        {}", cfg.kmsKeyId);
            logger.info("  - Resource Group: {}", cfg.resourceGroupName);
            logger.info("Summary:");
            logger.info("  - AWS Account ID: {}", accountId);
            logger.info("  - Region:         {}", region);
            logger.info("============================================================");
            logger.info("To destroy infrastructure, run: cd ../cdk && npx cdk destroy");

        } finally {
            stsClient.close();
            xrayClient.close();
        }
    }
}
