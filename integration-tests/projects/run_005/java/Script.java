import software.amazon.awssdk.regions.Region;
import software.amazon.awssdk.services.sts.StsClient;
import software.amazon.awssdk.services.sts.model.*;
import software.amazon.awssdk.services.secretsmanager.SecretsManagerClient;
import software.amazon.awssdk.services.secretsmanager.model.*;
import software.amazon.awssdk.services.sns.SnsClient;
import software.amazon.awssdk.services.sns.model.*;

import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.node.ObjectNode;

import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

import java.io.File;
import java.util.*;

public class Script {
    private static final Logger logger = LoggerFactory.getLogger(Script.class);
    private static final ObjectMapper objectMapper = new ObjectMapper();

    // ── Config loading ─────────────────────────────────────────────────────────

    static class RunConfig {
        public String topicArn;
        public String secretName;
        public String secretArn;
        public String kmsKeyId;
        public String kmsKeyArn;
        public String repoName;
        public String cloneUrl;
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

            logger.info("Starting Secure Repository Monitoring...");
            logger.info("Using SNS topic:  {}", cfg.topicArn);
            logger.info("Using secret:     {}", cfg.secretName);
            logger.info("Using repo:       {}", cfg.repoName);
            logger.info("Using region:     {}", region);

            runSecureRepoMonitoring(cfg, region);

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

    private static ObjectNode retrieveSecret(SecretsManagerClient secretsClient, String secretName) throws Exception {
        GetSecretValueResponse response = secretsClient.getSecretValue(
                GetSecretValueRequest.builder()
                        .secretId(secretName)
                        .build());
        ObjectNode secretData = (ObjectNode) objectMapper.readTree(response.secretString());
        logger.info("Successfully retrieved and decrypted configuration from Secrets Manager");
        return secretData;
    }

    private static void sendNotification(SnsClient snsClient, String topicArn,
                                         String repoName, String cloneUrl) throws Exception {
        ObjectNode message = objectMapper.createObjectNode();
        message.put("default", "Secure repository '" + repoName + "' is configured and ready.");
        message.put("email", String.format(
                "Repository Monitoring Alert\n\n" +
                "Repository: %s\n" +
                "Clone URL: %s\n\n" +
                "Security features: KMS encryption, SNS notifications, Secrets Manager integration.",
                repoName, cloneUrl));

        String messageJson = objectMapper.writeValueAsString(message);

        snsClient.publish(PublishRequest.builder()
                .topicArn(topicArn)
                .message(messageJson)
                .messageStructure("json")
                .subject("Repository Ready: " + repoName)
                .build());
        logger.info("Notification sent successfully");
    }

    // ── Main logic ────────────────────────────────────────────────────────────

    private static void runSecureRepoMonitoring(RunConfig cfg, Region region) throws Exception {
        StsClient stsClient               = StsClient.builder().region(region).build();
        SecretsManagerClient secretsClient = SecretsManagerClient.builder().region(region).build();
        SnsClient snsClient               = SnsClient.builder().region(region).build();

        try {
            // 1. Get account ID
            logger.info("Getting AWS account information...");
            String accountId = getAwsAccountId(stsClient);
            logger.info("AWS Account ID: {}", accountId);

            // 2. Retrieve and verify secret from Secrets Manager
            logger.info("Retrieving configuration from Secrets Manager...");
            ObjectNode secretData = retrieveSecret(secretsClient, cfg.secretName);
            String verifiedRepoName = secretData.has("repository_name")
                    ? secretData.get("repository_name").asText()
                    : cfg.repoName;
            logger.info("Verified configuration for repository: {}", verifiedRepoName);

            // 3. Send notification via SNS
            logger.info("Sending repository notification via SNS...");
            sendNotification(snsClient, cfg.topicArn, cfg.repoName, cfg.cloneUrl);

            logger.info("============================================================");
            logger.info("APPLICATION COMPLETED SUCCESSFULLY!");
            logger.info("============================================================");
            logger.info("Resources used:");
            logger.info("  - SNS Topic:  {}", cfg.topicArn);
            logger.info("  - Secret:     {}", cfg.secretName);
            logger.info("  - Repo:       {}", cfg.repoName);
            logger.info("  - Clone URL:  {}", cfg.cloneUrl);
            logger.info("============================================================");
            logger.info("To destroy infrastructure, run: cd ../cdk && npx cdk destroy");

        } finally {
            stsClient.close();
            secretsClient.close();
            snsClient.close();
        }
    }
}
