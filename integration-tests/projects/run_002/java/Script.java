import software.amazon.awssdk.regions.Region;
import software.amazon.awssdk.services.s3.S3Client;
import software.amazon.awssdk.services.s3.model.*;
import software.amazon.awssdk.services.sqs.SqsClient;
import software.amazon.awssdk.services.sqs.model.*;
import software.amazon.awssdk.services.cloudwatch.CloudWatchClient;
import software.amazon.awssdk.services.cloudwatch.model.*;
import software.amazon.awssdk.services.sts.StsClient;
import software.amazon.awssdk.services.sts.model.*;
import software.amazon.awssdk.core.sync.RequestBody;

import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.node.ObjectNode;

import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

import java.io.File;
import java.time.Instant;
import java.util.*;

public class Script {
    private static final Logger logger = LoggerFactory.getLogger(Script.class);
    private static final ObjectMapper objectMapper = new ObjectMapper();

    // ── Config loading ─────────────────────────────────────────────────────────

    static class RunConfig {
        public String bucketName;
        public String queueUrl;
        public String region;
    }

    private static RunConfig loadConfig() throws Exception {
        // config.json lives one directory above the java/ directory (at the run level).
        // We resolve relative to the working directory (which is the java/ dir when
        // invoked via `mvn exec:java -f pom.xml`).
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

            logger.info("Starting AWS File Processing Monitoring System...");
            logger.info("Using bucket:    {}", cfg.bucketName);
            logger.info("Using queue URL: {}", cfg.queueUrl);
            logger.info("Using region:    {}", region);

            processFileMonitoringSystem(cfg.bucketName, cfg.queueUrl, region);

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

    private static void uploadFileToS3(S3Client s3Client, String bucketName,
                                       String fileContent, String fileKey) {
        s3Client.putObject(
                PutObjectRequest.builder()
                        .bucket(bucketName)
                        .key(fileKey)
                        .contentType("application/json")
                        .build(),
                RequestBody.fromString(fileContent));
    }

    private static String sendSqsMessage(SqsClient sqsClient, String queueUrl,
                                         String messageBody) {
        SendMessageResponse response = sqsClient.sendMessage(
                SendMessageRequest.builder()
                        .queueUrl(queueUrl)
                        .messageBody(messageBody)
                        .build());
        return response.messageId();
    }

    private static List<Message> receiveSqsMessages(SqsClient sqsClient,
                                                    String queueUrl, int maxMessages) {
        ReceiveMessageResponse response = sqsClient.receiveMessage(
                ReceiveMessageRequest.builder()
                        .queueUrl(queueUrl)
                        .maxNumberOfMessages(maxMessages)
                        .waitTimeSeconds(5)
                        .build());
        return response.messages();
    }

    private static void deleteSqsMessage(SqsClient sqsClient, String queueUrl,
                                         String receiptHandle) {
        sqsClient.deleteMessage(
                DeleteMessageRequest.builder()
                        .queueUrl(queueUrl)
                        .receiptHandle(receiptHandle)
                        .build());
    }

    private static void putCloudWatchMetric(CloudWatchClient cloudWatchClient,
                                            String namespace, String metricName,
                                            double value, StandardUnit unit) {
        MetricDatum datum = MetricDatum.builder()
                .metricName(metricName)
                .value(value)
                .unit(unit)
                .timestamp(Instant.now())
                .build();

        cloudWatchClient.putMetricData(
                PutMetricDataRequest.builder()
                        .namespace(namespace)
                        .metricData(datum)
                        .build());
    }

    // ── Main logic ────────────────────────────────────────────────────────────

    private static void processFileMonitoringSystem(String bucketName, String queueUrl,
                                                    Region region) throws Exception {
        S3Client s3Client                 = S3Client.builder().region(region).build();
        SqsClient sqsClient               = SqsClient.builder().region(region).build();
        CloudWatchClient cloudWatchClient = CloudWatchClient.builder().region(region).build();
        StsClient stsClient               = StsClient.builder().region(region).build();

        try {
            logger.info("Getting AWS account information...");
            String accountId = getAwsAccountId(stsClient);
            logger.info("Using AWS Account ID: {}", accountId);

            List<Map<String, Object>> filesToProcess = Arrays.asList(
                    createFileInfo("data1.json", 1024, "json"),
                    createFileInfo("data2.json", 2048, "json"),
                    createFileInfo("data3.json", 512,  "json")
            );

            int processedFiles = 0;
            int totalSize = 0;

            for (Map<String, Object> fileInfo : filesToProcess) {
                String filename = (String) fileInfo.get("filename");
                int    size     = (Integer) fileInfo.get("size");
                String type     = (String) fileInfo.get("type");

                ObjectNode fileContent = objectMapper.createObjectNode();
                fileContent.put("filename",     filename);
                fileContent.put("processed_at", Instant.now().toString());
                fileContent.put("size",         size);
                fileContent.put("type",         type);
                fileContent.put("processed_by", "file-monitoring-system");

                logger.info("Uploading {} to S3...", filename);
                uploadFileToS3(s3Client, bucketName,
                        objectMapper.writerWithDefaultPrettyPrinter()
                                .writeValueAsString(fileContent),
                        filename);

                ObjectNode sqsMessage = objectMapper.createObjectNode();
                sqsMessage.put("action",     "file_processed");
                sqsMessage.put("filename",   filename);
                sqsMessage.put("bucket",     bucketName);
                sqsMessage.put("size",       size);
                sqsMessage.put("timestamp",  Instant.now().toString());
                sqsMessage.put("account_id", accountId);

                logger.info("Sending processing notification to SQS...");
                String messageId = sendSqsMessage(sqsClient, queueUrl,
                        objectMapper.writeValueAsString(sqsMessage));
                logger.info("SQS message sent with ID: {}", messageId);

                processedFiles++;
                totalSize += size;

                logger.info("Sending metrics to CloudWatch...");
                putCloudWatchMetric(cloudWatchClient, "FileProcessing",
                        "FilesProcessed", 1, StandardUnit.COUNT);
                putCloudWatchMetric(cloudWatchClient, "FileProcessing",
                        "BytesProcessed", size, StandardUnit.BYTES);

                Thread.sleep(1000);
            }

            logger.info("Reading processing notifications from SQS...");
            List<Message> messages = receiveSqsMessages(sqsClient, queueUrl, 10);

            for (Message message : messages) {
                ObjectNode messageBody =
                        (ObjectNode) objectMapper.readTree(message.body());
                String filename = messageBody.get("filename").asText();
                int    size     = messageBody.get("size").asInt();
                logger.info("Processing notification: {} ({} bytes)", filename, size);

                deleteSqsMessage(sqsClient, queueUrl, message.receiptHandle());
                logger.info("Notification processed and removed from queue");
            }

            logger.info("Sending summary metrics to CloudWatch...");
            putCloudWatchMetric(cloudWatchClient, "FileProcessing",
                    "TotalFilesProcessed", processedFiles, StandardUnit.COUNT);
            putCloudWatchMetric(cloudWatchClient, "FileProcessing",
                    "TotalBytesProcessed", totalSize, StandardUnit.BYTES);

            logger.info("File processing monitoring completed!");
            logger.info("Total files processed: {}", processedFiles);
            logger.info("Total bytes processed: {}", totalSize);
            logger.info("S3 bucket:   {}", bucketName);
            logger.info("SQS queue URL: {}", queueUrl);

            logger.info("============================================================");
            logger.info("APPLICATION COMPLETED SUCCESSFULLY!");
            logger.info("============================================================");
            logger.info("Resources used:");
            logger.info("  - S3 Bucket:          {}", bucketName);
            logger.info("  - SQS Queue URL:      {}", queueUrl);
            logger.info("  - CloudWatch Metrics: FileProcessing namespace");
            logger.info("Summary:");
            logger.info("  - Files processed:    {}", processedFiles);
            logger.info("  - Total bytes:        {}", totalSize);
            logger.info("============================================================");

        } finally {
            s3Client.close();
            sqsClient.close();
            cloudWatchClient.close();
            stsClient.close();
        }
    }

    private static Map<String, Object> createFileInfo(String filename, int size, String type) {
        Map<String, Object> fileInfo = new HashMap<>();
        fileInfo.put("filename", filename);
        fileInfo.put("size",     size);
        fileInfo.put("type",     type);
        return fileInfo;
    }
}
