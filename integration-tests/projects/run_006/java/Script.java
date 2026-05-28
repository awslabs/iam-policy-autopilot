import software.amazon.awssdk.core.ResponseBytes;
import software.amazon.awssdk.core.sync.RequestBody;
import software.amazon.awssdk.regions.Region;
import software.amazon.awssdk.services.s3.S3Client;
import software.amazon.awssdk.services.s3.model.*;
import software.amazon.awssdk.services.dynamodb.DynamoDbClient;
import software.amazon.awssdk.services.dynamodb.model.*;
import software.amazon.awssdk.services.cloudwatchlogs.CloudWatchLogsClient;
import software.amazon.awssdk.services.cloudwatchlogs.model.*;
import software.amazon.awssdk.services.sts.StsClient;
import software.amazon.awssdk.services.sts.model.*;

import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.node.ObjectNode;

import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

import java.io.File;
import java.nio.charset.StandardCharsets;
import java.nio.file.*;
import java.security.MessageDigest;
import java.time.Instant;
import java.time.LocalDate;
import java.time.format.DateTimeFormatter;
import java.util.*;

public class Script {
    private static final Logger logger = LoggerFactory.getLogger(Script.class);
    private static final ObjectMapper objectMapper = new ObjectMapper();

    // ── Config loading ─────────────────────────────────────────────────────────

    static class RunConfig {
        public String bucketName;
        public String tableName;
        public String kmsKeyId;
        public String kmsKeyArn;
        public String kmsAlias;
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

            logger.info("Starting Secure Document Management System...");
            logger.info("Using bucket:    {}", cfg.bucketName);
            logger.info("Using table:     {}", cfg.tableName);
            logger.info("Using KMS key:   {}", cfg.kmsKeyId);
            logger.info("Using log group: {}", cfg.logGroupName);
            logger.info("Using region:    {}", region);

            runDemo(cfg, region);

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

    private static String sha256Hex(byte[] input) throws Exception {
        MessageDigest digest = MessageDigest.getInstance("SHA-256");
        byte[] hash = digest.digest(input);
        StringBuilder sb = new StringBuilder();
        for (byte b : hash) {
            sb.append(String.format("%02x", b));
        }
        return sb.toString();
    }

    /**
     * Upload document to S3 and store metadata in DynamoDB.
     * Returns the generated documentId.
     */
    private static String uploadDocument(
            S3Client s3Client,
            DynamoDbClient dynamoDbClient,
            String bucketName,
            String tableName,
            String kmsKeyId,
            byte[] fileContent,
            String documentName) throws Exception {

        String fileHash = sha256Hex(fileContent);
        String rawId = sha256Hex((documentName + "_" + System.currentTimeMillis())
                .getBytes(StandardCharsets.UTF_8));
        String documentId = rawId.substring(0, 16);
        String s3Key = "documents/" + documentId + "/" + documentName;

        // S3 PutObject with KMS encryption
        Map<String, String> metadata = new HashMap<>();
        metadata.put("document-id", documentId);
        metadata.put("original-name", documentName);

        s3Client.putObject(
                PutObjectRequest.builder()
                        .bucket(bucketName)
                        .key(s3Key)
                        .serverSideEncryption(ServerSideEncryption.AWS_KMS)
                        .ssekmsKeyId(kmsKeyId)
                        .metadata(metadata)
                        .build(),
                RequestBody.fromBytes(fileContent));

        logger.info("Uploaded to s3://{}/{}", bucketName, s3Key);

        // DynamoDB PutItem — store metadata
        Map<String, AttributeValue> item = new HashMap<>();
        item.put("document_id",      AttributeValue.builder().s(documentId).build());
        item.put("document_name",    AttributeValue.builder().s(documentName).build());
        item.put("s3_bucket",        AttributeValue.builder().s(bucketName).build());
        item.put("s3_key",           AttributeValue.builder().s(s3Key).build());
        item.put("file_hash",        AttributeValue.builder().s(fileHash).build());
        item.put("file_size",        AttributeValue.builder().n(String.valueOf(fileContent.length)).build());
        item.put("upload_timestamp", AttributeValue.builder().s(Instant.now().toString()).build());
        item.put("status",           AttributeValue.builder().s("active").build());

        dynamoDbClient.putItem(PutItemRequest.builder()
                .tableName(tableName)
                .item(item)
                .build());

        logger.info("Stored metadata in DynamoDB for document_id={}", documentId);
        return documentId;
    }

    /**
     * Log an operation to CloudWatch Logs.
     */
    private static void logOperation(
            CloudWatchLogsClient logsClient,
            String logGroupName,
            String operation,
            String documentId,
            String documentName,
            String status) throws Exception {
        String logStreamName = "document-operations-"
                + LocalDate.now().format(DateTimeFormatter.ISO_LOCAL_DATE);

        // CreateLogStream (ignore ResourceAlreadyExistsException)
        try {
            logsClient.createLogStream(CreateLogStreamRequest.builder()
                    .logGroupName(logGroupName)
                    .logStreamName(logStreamName)
                    .build());
        } catch (ResourceAlreadyExistsException e) {
            // Stream already exists — continue
        }

        ObjectNode logEntry = objectMapper.createObjectNode();
        logEntry.put("timestamp",     Instant.now().toString());
        logEntry.put("operation",     operation);
        logEntry.put("document_id",   documentId);
        logEntry.put("document_name", documentName);
        logEntry.put("status",        status);

        InputLogEvent logEvent = InputLogEvent.builder()
                .timestamp(System.currentTimeMillis())
                .message(objectMapper.writeValueAsString(logEntry))
                .build();

        logsClient.putLogEvents(PutLogEventsRequest.builder()
                .logGroupName(logGroupName)
                .logStreamName(logStreamName)
                .logEvents(logEvent)
                .build());

        logger.info("Logged {} operation to CloudWatch", operation);
    }

    /**
     * Scan DynamoDB table and return all document items.
     */
    private static List<Map<String, AttributeValue>> listDocuments(
            DynamoDbClient dynamoDbClient, String tableName) {
        ScanResponse response = dynamoDbClient.scan(ScanRequest.builder()
                .tableName(tableName)
                .build());
        List<Map<String, AttributeValue>> docs = response.items();
        logger.info("Found {} document(s) in DynamoDB", docs.size());
        return docs;
    }

    /**
     * Download a document from S3, verify integrity, and save to downloadPath.
     * Returns the document name.
     */
    private static String downloadDocument(
            S3Client s3Client,
            DynamoDbClient dynamoDbClient,
            String bucketName,
            String tableName,
            String documentId,
            String downloadPath) throws Exception {

        // DynamoDB GetItem — fetch metadata
        Map<String, AttributeValue> key = new HashMap<>();
        key.put("document_id", AttributeValue.builder().s(documentId).build());

        GetItemResponse getItemResponse = dynamoDbClient.getItem(GetItemRequest.builder()
                .tableName(tableName)
                .key(key)
                .build());

        if (!getItemResponse.hasItem() || getItemResponse.item().isEmpty()) {
            throw new IllegalArgumentException("Document not found: " + documentId);
        }

        Map<String, AttributeValue> item = getItemResponse.item();
        String s3Key      = item.get("s3_key").s();
        String storedHash = item.get("file_hash").s();
        String docName    = item.get("document_name").s();

        // S3 GetObject
        ResponseBytes<GetObjectResponse> objectBytes = s3Client.getObjectAsBytes(
                GetObjectRequest.builder()
                        .bucket(bucketName)
                        .key(s3Key)
                        .build());

        byte[] fileContent = objectBytes.asByteArray();

        // Integrity check
        String fileHash = sha256Hex(fileContent);
        if (!fileHash.equals(storedHash)) {
            throw new IllegalStateException("File integrity check failed");
        }

        Files.write(Paths.get(downloadPath), fileContent);
        logger.info("Downloaded document to {}", downloadPath);
        return docName;
    }

    // ── Main logic ────────────────────────────────────────────────────────────

    private static void runDemo(RunConfig cfg, Region region) throws Exception {
        S3Client s3Client                  = S3Client.builder().region(region).build();
        DynamoDbClient dynamoDbClient      = DynamoDbClient.builder().region(region).build();
        CloudWatchLogsClient logsClient    = CloudWatchLogsClient.builder().region(region).build();
        StsClient stsClient                = StsClient.builder().region(region).build();

        try {
            // 1. STS GetCallerIdentity
            logger.info("Getting AWS account information...");
            String accountId = getAwsAccountId(stsClient);
            logger.info("AWS Account ID: {}", accountId);

            // 2. Create sample document
            String sampleContent = "This is a sample document for testing the secure document management system.";
            String samplePath = "/tmp/sample_document.txt";
            Files.write(Paths.get(samplePath), sampleContent.getBytes(StandardCharsets.UTF_8));
            logger.info("Created sample document at {}", samplePath);

            // 3. S3 PutObject + DynamoDB PutItem
            logger.info("Uploading document...");
            byte[] fileContent = sampleContent.getBytes(StandardCharsets.UTF_8);
            String documentId = uploadDocument(s3Client, dynamoDbClient,
                    cfg.bucketName, cfg.tableName, cfg.kmsKeyId,
                    fileContent, "sample_document.txt");

            // 4. CloudWatch Logs — log UPLOAD
            logOperation(logsClient, cfg.logGroupName,
                    "UPLOAD", documentId, "sample_document.txt", "SUCCESS");

            // 5. DynamoDB Scan — list all documents
            List<Map<String, AttributeValue>> docs = listDocuments(dynamoDbClient, cfg.tableName);

            // 6. S3 GetObject + DynamoDB GetItem — download
            logger.info("Downloading document...");
            String downloadPath = "/tmp/downloaded_sample.txt";
            String docName = downloadDocument(s3Client, dynamoDbClient,
                    cfg.bucketName, cfg.tableName, documentId, downloadPath);

            // 7. CloudWatch Logs — log DOWNLOAD
            logOperation(logsClient, cfg.logGroupName,
                    "DOWNLOAD", documentId, docName, "SUCCESS");

            logger.info("============================================================");
            logger.info("APPLICATION COMPLETED SUCCESSFULLY!");
            logger.info("============================================================");
            logger.info("Resources used:");
            logger.info("  - S3 Bucket:   {}", cfg.bucketName);
            logger.info("  - DynamoDB:    {}", cfg.tableName);
            logger.info("  - Log Group:   {}", cfg.logGroupName);
            logger.info("Summary:");
            logger.info("  - Document ID:    {}", documentId);
            logger.info("  - Total docs:     {}", docs.size());
            logger.info("============================================================");
            logger.info("To destroy infrastructure, run: cd ../cdk && npx cdk destroy");

        } finally {
            s3Client.close();
            dynamoDbClient.close();
            logsClient.close();
            stsClient.close();
        }
    }
}
