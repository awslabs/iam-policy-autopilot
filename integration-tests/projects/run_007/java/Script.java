import software.amazon.awssdk.core.sync.RequestBody;
import software.amazon.awssdk.regions.Region;
import software.amazon.awssdk.services.s3.S3Client;
import software.amazon.awssdk.services.s3.model.PutObjectRequest;
import software.amazon.awssdk.services.ses.SesClient;
import software.amazon.awssdk.services.ses.model.Body;
import software.amazon.awssdk.services.ses.model.Content;
import software.amazon.awssdk.services.ses.model.Destination;
import software.amazon.awssdk.services.ses.model.Message;
import software.amazon.awssdk.services.ses.model.SendEmailRequest;
import software.amazon.awssdk.services.ses.model.SesException;
import software.amazon.awssdk.services.sts.StsClient;
import software.amazon.awssdk.services.sts.model.GetCallerIdentityResponse;

import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.node.ObjectNode;

import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

import java.io.File;
import java.time.Instant;
import java.util.Random;

public class Script {
    private static final Logger logger = LoggerFactory.getLogger(Script.class);
    private static final ObjectMapper objectMapper = new ObjectMapper();

    // ── Config loading ─────────────────────────────────────────────────────────

    static class RunConfig {
        public String bucketName;
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

            logger.info("Starting AWS Comprehensive Monitoring System (data-plane)...");
            logger.info("Using bucket: {}", cfg.bucketName);
            logger.info("Using region: {}", region);

            runDemo(cfg, region);

        } catch (Exception e) {
            logger.error("Application failed: {}", e.getMessage());
            System.exit(1);
        }
    }

    // ── Data-plane helpers ─────────────────────────────────────────────────────

    private static void uploadSampleData(S3Client s3Client, String bucketName) throws Exception {
        Random random = new Random();

        ObjectNode systemMetrics = objectMapper.createObjectNode();
        systemMetrics.put("cpu_usage",    random.nextDouble() * 80 + 10);
        systemMetrics.put("memory_usage", random.nextDouble() * 60 + 20);
        systemMetrics.put("disk_usage",   random.nextDouble() * 40 + 30);

        ObjectNode appMetrics = objectMapper.createObjectNode();
        appMetrics.put("requests_per_second", random.nextInt(900) + 100);
        appMetrics.put("error_rate",          random.nextDouble() * 4.9 + 0.1);
        appMetrics.put("response_time",       random.nextDouble() * 400 + 100);

        ObjectNode sampleData = objectMapper.createObjectNode();
        sampleData.put("timestamp", Instant.now().toEpochMilli() / 1000.0);
        sampleData.set("system_metrics", systemMetrics);
        sampleData.set("application_metrics", appMetrics);

        String body = objectMapper.writerWithDefaultPrettyPrinter().writeValueAsString(sampleData);
        String key  = "monitoring-data/" + Instant.now().getEpochSecond() + ".json";

        s3Client.putObject(
                PutObjectRequest.builder()
                        .bucket(bucketName)
                        .key(key)
                        .contentType("application/json")
                        .build(),
                RequestBody.fromString(body));

        logger.info("Uploaded sample monitoring data to S3: s3://{}/{}", bucketName, key);
    }

    private static void sendNotificationEmail(SesClient sesClient,
                                               String senderEmail,
                                               String recipientEmail) {
        // Only tolerate MessageRejected (unverified addresses in sandbox).
        // All other errors — including AccessDeniedException — propagate so the
        // minimizer can detect missing permissions.
        try {
            String subject = "AWS Monitoring System - Setup Complete";
            String body    = "Your AWS monitoring system has been successfully set up.\n\n"
                           + "The system is now ready for use.";

            sesClient.sendEmail(SendEmailRequest.builder()
                    .source(senderEmail)
                    .destination(Destination.builder()
                            .toAddresses(recipientEmail)
                            .build())
                    .message(Message.builder()
                            .subject(Content.builder().data(subject).build())
                            .body(Body.builder()
                                    .text(Content.builder().data(body).build())
                                    .build())
                            .build())
                    .build());

            logger.info("Sent notification email to {}", recipientEmail);

        } catch (SesException e) {
            if ("MessageRejected".equals(e.awsErrorDetails().errorCode())) {
                logger.warn("SES SendEmail: email not verified (non-fatal)");
            } else {
                throw e; // Re-throw AccessDeniedException and other errors
            }
        }
    }

    // ── Main logic ────────────────────────────────────────────────────────────

    private static void runDemo(RunConfig cfg, Region region) throws Exception {
        S3Client  s3Client  = S3Client.builder().region(region).build();
        SesClient sesClient = SesClient.builder().region(region).build();
        StsClient stsClient = StsClient.builder().region(region).build();

        try {
            // 1. STS GetCallerIdentity — verify credentials
            GetCallerIdentityResponse identity = stsClient.getCallerIdentity();
            logger.info("AWS Account ID: {}", identity.account());

            // 2. S3 PutObject — upload monitoring data
            uploadSampleData(s3Client, cfg.bucketName);

            // 3. SES SendEmail — MessageRejected is tolerated; permission errors propagate
            sendNotificationEmail(sesClient, "test@example.com", "test@example.com");

            logger.info("============================================================");
            logger.info("APPLICATION COMPLETED SUCCESSFULLY!");
            logger.info("============================================================");
            logger.info("Resources used:");
            logger.info("  - S3 Bucket: {}", cfg.bucketName);
            logger.info("============================================================");
            logger.info("To destroy infrastructure, run: cd ../cdk && npx cdk destroy");

        } finally {
            s3Client.close();
            sesClient.close();
            stsClient.close();
        }
    }
}
