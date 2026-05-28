import software.amazon.awssdk.regions.Region;
import software.amazon.awssdk.services.s3.S3Client;
import software.amazon.awssdk.services.s3.model.*;
import software.amazon.awssdk.services.sfn.SfnClient;
import software.amazon.awssdk.services.sfn.model.*;
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
        public String kmsKeyId;
        public String kmsKeyArn;
        public String stateMachineArn;
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

            logger.info("Starting AWS Data Processing Pipeline...");
            logger.info("Using bucket:        {}", cfg.bucketName);
            logger.info("Using KMS key:       {}", cfg.kmsKeyId);
            logger.info("Using state machine: {}", cfg.stateMachineArn);
            logger.info("Using region:        {}", region);

            runDataPipeline(cfg, region);

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

    private static String uploadSampleData(S3Client s3Client, String bucketName,
                                           String kmsKeyId, long timestamp) throws Exception {
        String key = "data/sample-" + timestamp + ".json";

        ObjectNode sampleData = objectMapper.createObjectNode();
        sampleData.put("timestamp", timestamp);
        sampleData.put("data", "Sample data for processing pipeline");
        sampleData.put("processed", false);

        String body = objectMapper.writeValueAsString(sampleData);

        s3Client.putObject(
                PutObjectRequest.builder()
                        .bucket(bucketName)
                        .key(key)
                        .contentType("application/json")
                        .serverSideEncryption(ServerSideEncryption.AWS_KMS)
                        .ssekmsKeyId(kmsKeyId)
                        .build(),
                RequestBody.fromString(body));

        logger.info("Uploaded sample data to s3://{}/{}", bucketName, key);
        return key;
    }

    private static String startPipelineExecution(SfnClient sfnClient, String stateMachineArn,
                                                  String bucketName, long timestamp) throws Exception {
        ObjectNode input = objectMapper.createObjectNode();
        input.put("bucket", bucketName);
        input.put("timestamp", timestamp);

        StartExecutionResponse response = sfnClient.startExecution(
                StartExecutionRequest.builder()
                        .stateMachineArn(stateMachineArn)
                        .input(objectMapper.writeValueAsString(input))
                        .build());

        String executionArn = response.executionArn();
        logger.info("Started execution: {}", executionArn);
        return executionArn;
    }

    private static String pollExecution(SfnClient sfnClient, String executionArn,
                                        int timeoutSeconds) throws Exception {
        Set<String> terminalStatuses = new HashSet<>(
                Arrays.asList("SUCCEEDED", "FAILED", "TIMED_OUT", "ABORTED"));
        long deadline = System.currentTimeMillis() + timeoutSeconds * 1000L;

        while (System.currentTimeMillis() < deadline) {
            DescribeExecutionResponse response = sfnClient.describeExecution(
                    DescribeExecutionRequest.builder()
                            .executionArn(executionArn)
                            .build());
            String status = response.statusAsString();
            logger.info("Execution status: {}", status);
            if (terminalStatuses.contains(status)) {
                return status;
            }
            Thread.sleep(5000);
        }
        throw new RuntimeException(
                "Execution did not reach terminal state within " + timeoutSeconds + "s");
    }

    private static void putPipelineMetrics(CloudWatchClient cloudWatchClient) {
        String namespace = "DataProcessingPipeline";
        Instant now = Instant.now();

        List<MetricDatum> metrics = Arrays.asList(
                MetricDatum.builder()
                        .metricName("PipelineExecutions")
                        .value(1.0)
                        .unit(StandardUnit.COUNT)
                        .timestamp(now)
                        .build(),
                MetricDatum.builder()
                        .metricName("FilesProcessed")
                        .value(1.0)
                        .unit(StandardUnit.COUNT)
                        .timestamp(now)
                        .build()
        );

        cloudWatchClient.putMetricData(
                PutMetricDataRequest.builder()
                        .namespace(namespace)
                        .metricData(metrics)
                        .build());

        logger.info("Published metrics to CloudWatch namespace '{}'", namespace);
    }

    // ── Main logic ────────────────────────────────────────────────────────────

    private static void runDataPipeline(RunConfig cfg, Region region) throws Exception {
        S3Client s3Client                 = S3Client.builder().region(region).build();
        SfnClient sfnClient               = SfnClient.builder().region(region).build();
        CloudWatchClient cloudWatchClient = CloudWatchClient.builder().region(region).build();
        StsClient stsClient               = StsClient.builder().region(region).build();

        try {
            // 1. Get account ID
            logger.info("Getting AWS account information...");
            String accountId = getAwsAccountId(stsClient);
            logger.info("AWS Account ID: {}", accountId);

            // 2. Upload sample data with KMS encryption
            long timestamp = Instant.now().getEpochSecond();
            logger.info("Uploading sample data to S3 with KMS encryption...");
            String dataKey = uploadSampleData(s3Client, cfg.bucketName, cfg.kmsKeyId, timestamp);

            // 3. Start Step Functions execution
            logger.info("Starting Step Functions pipeline execution...");
            String executionArn = startPipelineExecution(
                    sfnClient, cfg.stateMachineArn, cfg.bucketName, timestamp);

            // 4. Poll for completion (60s timeout, 5s interval)
            logger.info("Polling for execution completion (timeout: 60s)...");
            String finalStatus = pollExecution(sfnClient, executionArn, 60);
            logger.info("Execution finished with status: {}", finalStatus);

            // 5. Put custom CloudWatch metrics
            logger.info("Publishing custom CloudWatch metrics...");
            putPipelineMetrics(cloudWatchClient);

            logger.info("============================================================");
            logger.info("APPLICATION COMPLETED SUCCESSFULLY!");
            logger.info("============================================================");
            logger.info("Resources used:");
            logger.info("  - S3 Bucket:          {}", cfg.bucketName);
            logger.info("  - Data key:           {}", dataKey);
            logger.info("  - State Machine:      {}", cfg.stateMachineArn);
            logger.info("  - CloudWatch Metrics: DataProcessingPipeline namespace");
            logger.info("Summary:");
            logger.info("  - Execution ARN:      {}", executionArn);
            logger.info("  - Execution status:   {}", finalStatus);
            logger.info("============================================================");

        } finally {
            s3Client.close();
            sfnClient.close();
            cloudWatchClient.close();
            stsClient.close();
        }
    }
}
