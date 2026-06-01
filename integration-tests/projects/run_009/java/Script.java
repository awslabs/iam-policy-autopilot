/**
 * AWS Compliance Monitoring System — data-plane script (CDK-refactored)
 *
 * Infrastructure (KMS key, S3 bucket) is provisioned by the CDK stack in
 * ../cdk/lib/stack.ts.  Deploy it first:
 *
 *   cd ../cdk && bash deploy.sh
 *
 * That writes ../config.json with the stack outputs.  Then just run:
 *
 *   mvn exec:java -Dexec.mainClass=Script
 *
 * Services used (data-plane only):
 *   s3              : GetBucketLocation, PutObject (SSE-KMS)
 *   glue            : GetDatabase, CreateDatabase, GetTable, CreateTable
 *   athena          : StartQueryExecution, GetQueryExecution, GetQueryResults
 *   cloudwatch      : PutMetricData
 *   organizations   : ListAccounts (graceful fallback if not in org)
 *   sts             : GetCallerIdentity (fallback for org data)
 */

import software.amazon.awssdk.core.sync.RequestBody;
import software.amazon.awssdk.regions.Region;
import software.amazon.awssdk.services.s3.S3Client;
import software.amazon.awssdk.services.s3.model.*;
import software.amazon.awssdk.services.athena.AthenaClient;
import software.amazon.awssdk.services.athena.model.*;
import software.amazon.awssdk.services.cloudwatch.CloudWatchClient;
import software.amazon.awssdk.services.cloudwatch.model.*;
import software.amazon.awssdk.services.organizations.OrganizationsClient;
import software.amazon.awssdk.services.organizations.model.*;
import software.amazon.awssdk.services.sts.StsClient;
import software.amazon.awssdk.services.sts.model.*;
import software.amazon.awssdk.services.glue.GlueClient;
import software.amazon.awssdk.services.glue.model.Column;
import software.amazon.awssdk.services.glue.model.CreateDatabaseRequest;
import software.amazon.awssdk.services.glue.model.CreateTableRequest;
import software.amazon.awssdk.services.glue.model.DatabaseInput;
import software.amazon.awssdk.services.glue.model.EntityNotFoundException;
import software.amazon.awssdk.services.glue.model.GetDatabaseRequest;
import software.amazon.awssdk.services.glue.model.GetTableRequest;
import software.amazon.awssdk.services.glue.model.PartitionInput;
import software.amazon.awssdk.services.glue.model.BatchCreatePartitionRequest;
import software.amazon.awssdk.services.glue.model.DeletePartitionRequest;
import software.amazon.awssdk.services.glue.model.GetPartitionsRequest;
import software.amazon.awssdk.services.glue.model.UpdateTableRequest;
import software.amazon.awssdk.services.glue.model.SerDeInfo;
import software.amazon.awssdk.services.glue.model.StorageDescriptor;
import software.amazon.awssdk.services.glue.model.TableInput;

import com.fasterxml.jackson.databind.ObjectMapper;

import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

import java.io.File;
import java.nio.charset.StandardCharsets;
import java.time.Instant;
import java.time.format.DateTimeFormatter;
import java.util.*;

public class Script {
    private static final Logger logger = LoggerFactory.getLogger(Script.class);
    private static final ObjectMapper objectMapper = new ObjectMapper();

    // ── Config loading ─────────────────────────────────────────────────────────

    static class RunConfig {
        public String bucketName;
        public String kmsKeyId;
        public String kmsKeyArn;
        public String region;
    }

    private static RunConfig loadConfig() throws Exception {
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

            logger.info("Starting AWS Compliance Monitoring System...");
            logger.info("Using bucket:  {}", cfg.bucketName);
            logger.info("Using KMS key: {}", cfg.kmsKeyId);
            logger.info("Using region:  {}", region);

            runMonitoring(cfg, region);

        } catch (Exception e) {
            logger.error("Application failed: {}", e.getMessage(), e);
            System.exit(1);
        }
    }

    // ── Account info ───────────────────────────────────────────────────────────

    static class AccountInfo {
        public String account_id;
        public String account_name;
        public String email;
        public String status;
        public String joined_method;
        public String joined_timestamp;
        public String collection_time;

        public AccountInfo(String accountId, String accountName, String email,
                           String status, String joinedMethod, String joinedTimestamp) {
            this.account_id = accountId;
            this.account_name = accountName;
            this.email = email;
            this.status = status;
            this.joined_method = joinedMethod;
            this.joined_timestamp = joinedTimestamp;
            this.collection_time = Instant.now().toString();
        }
    }

    // ── Collect organization data ──────────────────────────────────────────────

    private static List<AccountInfo> collectOrganizationData(
            OrganizationsClient orgClient, StsClient stsClient) {

        logger.info("Collecting organization data...");
        List<AccountInfo> accountsData = new ArrayList<>();

        try {
            ListAccountsRequest request = ListAccountsRequest.builder().build();
            boolean hasMore = true;
            String nextToken = null;

            while (hasMore) {
                ListAccountsRequest.Builder reqBuilder = ListAccountsRequest.builder();
                if (nextToken != null) {
                    reqBuilder.nextToken(nextToken);
                }
                ListAccountsResponse response = orgClient.listAccounts(reqBuilder.build());

                for (Account account : response.accounts()) {
                    accountsData.add(new AccountInfo(
                        account.id(),
                        account.name(),
                        account.email(),
                        account.statusAsString(),
                        account.joinedMethodAsString(),
                        account.joinedTimestamp() != null
                            ? DateTimeFormatter.ISO_INSTANT.format(account.joinedTimestamp())
                            : Instant.now().toString()
                    ));
                }

                nextToken = response.nextToken();
                hasMore = (nextToken != null);
            }

        } catch (OrganizationsException e) {
            String code = e.awsErrorDetails().errorCode();
            if ("AWSOrganizationsNotInUseException".equals(code) ||
                "AccessDeniedException".equals(code)) {
                logger.warn("Organizations not available, using current account only");
                GetCallerIdentityResponse identity = stsClient.getCallerIdentity();
                accountsData.add(new AccountInfo(
                    identity.account(),
                    "Current Account",
                    "unknown@example.com",
                    "ACTIVE",
                    "CREATED",
                    Instant.now().toString()
                ));
            } else {
                throw e;
            }
        }

        logger.info("Collected data for {} accounts", accountsData.size());
        return accountsData;
    }

    // ── Verify S3 bucket (grants s3:GetBucketLocation for Athena) ─────────────

    private static void verifyS3Bucket(S3Client s3Client, String bucketName) {
        GetBucketLocationResponse locationResponse = s3Client.getBucketLocation(
            GetBucketLocationRequest.builder().bucket(bucketName).build()
        );
        String location = locationResponse.locationConstraintAsString();
        if (location == null || location.isEmpty()) location = "us-east-1";
        logger.info("Bucket location: {}", location);
    }

    // ── Upload data to S3 ──────────────────────────────────────────────────────

    private static String uploadDataToS3(S3Client s3Client, String bucketName,
            String kmsKeyId, List<AccountInfo> data) throws Exception {

        logger.info("Uploading data to S3...");

        StringBuilder sb = new StringBuilder();
        for (AccountInfo record : data) {
            sb.append(objectMapper.writeValueAsString(record)).append("\n");
        }
        byte[] content = sb.toString().getBytes(StandardCharsets.UTF_8);

        Instant now = Instant.now();
        Calendar cal = Calendar.getInstance();
        String key = String.format(
            "compliance-data/year=%d/month=%02d/day=%02d/accounts_%d.json",
            cal.get(Calendar.YEAR),
            cal.get(Calendar.MONTH) + 1,
            cal.get(Calendar.DAY_OF_MONTH),
            now.getEpochSecond()
        );

        s3Client.putObject(
            PutObjectRequest.builder()
                .bucket(bucketName)
                .key(key)
                .contentType("application/json")
                .serverSideEncryption(ServerSideEncryption.AWS_KMS)
                .ssekmsKeyId(kmsKeyId)
                .build(),
            RequestBody.fromBytes(content)
        );

        logger.info("Uploaded data to S3: s3://{}/{}", bucketName, key);
        return key;
    }

    // ── Athena helpers ─────────────────────────────────────────────────────────

    private static String executeAthenaQuery(AthenaClient athenaClient,
            String query, String database, String bucketName, String kmsKeyId) {

        StartQueryExecutionRequest.Builder reqBuilder = StartQueryExecutionRequest.builder()
            .queryString(query)
            .resultConfiguration(ResultConfiguration.builder()
                .outputLocation("s3://" + bucketName + "/query-results/")
                .encryptionConfiguration(
                    software.amazon.awssdk.services.athena.model.EncryptionConfiguration.builder()
                        .encryptionOption(EncryptionOption.SSE_KMS)
                        .kmsKey(kmsKeyId)
                        .build())
                .build());

        if (database != null && !database.isEmpty()) {
            reqBuilder.queryExecutionContext(
                QueryExecutionContext.builder().database(database).build());
        }

        StartQueryExecutionResponse response = athenaClient.startQueryExecution(reqBuilder.build());
        return response.queryExecutionId();
    }

    private static void waitForQueryCompletion(AthenaClient athenaClient,
            String queryExecutionId) throws Exception {

        long maxWaitMs = 5 * 60 * 1000L;
        long startTime = System.currentTimeMillis();

        while (System.currentTimeMillis() - startTime < maxWaitMs) {
            GetQueryExecutionResponse response = athenaClient.getQueryExecution(
                GetQueryExecutionRequest.builder()
                    .queryExecutionId(queryExecutionId)
                    .build()
            );

            QueryExecutionState state = response.queryExecution().status().state();

            if (state == QueryExecutionState.SUCCEEDED) {
                return;
            } else if (state == QueryExecutionState.FAILED || state == QueryExecutionState.CANCELLED) {
                String reason = response.queryExecution().status().stateChangeReason();
                throw new Exception("Query " + state.toString().toLowerCase() + ": " +
                    (reason != null ? reason : "Unknown"));
            }

            Thread.sleep(5000);
        }

        throw new Exception("Query timed out after 5 minutes");
    }

    private static void setupGlueDatabase(GlueClient glueClient,
            String bucketName, String databaseName, String tableName) {

        logger.info("Setting up Glue database and table (Athena uses Glue Data Catalog)...");

        // Create database if it doesn't exist
        try {
            glueClient.getDatabase(GetDatabaseRequest.builder().name(databaseName).build());
            logger.info("Glue database '{}' already exists", databaseName);
        } catch (EntityNotFoundException e) {
            glueClient.createDatabase(CreateDatabaseRequest.builder()
                .databaseInput(DatabaseInput.builder()
                    .name(databaseName)
                    .description("Compliance monitoring database")
                    .build())
                .build());
            logger.info("Created Glue database '{}'", databaseName);
        }

        // Build table input (used for both create and update)
        TableInput tableInput = TableInput.builder()
            .name(tableName)
            .tableType("EXTERNAL_TABLE")
            .storageDescriptor(StorageDescriptor.builder()
                .columns(
                    Column.builder().name("account_id").type("string").build(),
                    Column.builder().name("account_name").type("string").build(),
                    Column.builder().name("email").type("string").build(),
                    Column.builder().name("status").type("string").build(),
                    Column.builder().name("joined_method").type("string").build(),
                    Column.builder().name("joined_timestamp").type("string").build(),
                    Column.builder().name("collection_time").type("string").build()
                )
                .location("s3://" + bucketName + "/compliance-data/")
                .inputFormat("org.apache.hadoop.mapred.TextInputFormat")
                .outputFormat("org.apache.hadoop.hive.ql.io.HiveIgnoreKeyTextOutputFormat")
                .serdeInfo(SerDeInfo.builder()
                    .serializationLibrary("org.apache.hive.hcatalog.data.JsonSerDe")
                    .build())
                .compressed(false)
                .build())
            .partitionKeys(
                Column.builder().name("year").type("string").build(),
                Column.builder().name("month").type("string").build(),
                Column.builder().name("day").type("string").build()
            )
            .parameters(Map.of(
                "has_encrypted_data", "true",
                "classification", "json"
            ))
            .build();

        // Create or update table (glue:GetTable + glue:CreateTable + glue:UpdateTable)
        try {
            glueClient.getTable(GetTableRequest.builder()
                .databaseName(databaseName)
                .name(tableName)
                .build());
            // Table exists — update its location to point to the current bucket
            logger.info("Glue table '{}.{}' already exists, updating location to current bucket", databaseName, tableName);
            glueClient.updateTable(UpdateTableRequest.builder()
                .databaseName(databaseName)
                .tableInput(tableInput)
                .build());
        } catch (EntityNotFoundException e) {
            glueClient.createTable(CreateTableRequest.builder()
                .databaseName(databaseName)
                .tableInput(tableInput)
                .build());
            logger.info("Created Glue table '{}.{}'", databaseName, tableName);
        }

        logger.info("Glue database and table ready");
    }

    private static void registerGluePartition(GlueClient glueClient,
            String bucketName, String databaseName, String tableName) {

        java.time.LocalDate today = java.time.LocalDate.now();
        int year = today.getYear();
        int month = today.getMonthValue();
        int day = today.getDayOfMonth();
        String yearStr = String.valueOf(year);
        String monthStr = String.format("%02d", month);
        String dayStr = String.format("%02d", day);
        String location = String.format("s3://%s/compliance-data/year=%s/month=%s/day=%s/",
            bucketName, yearStr, monthStr, dayStr);

        PartitionInput partitionInput = PartitionInput.builder()
            .values(yearStr, monthStr, dayStr)
            .storageDescriptor(StorageDescriptor.builder()
                .location(location)
                .inputFormat("org.apache.hadoop.mapred.TextInputFormat")
                .outputFormat("org.apache.hadoop.hive.ql.io.HiveIgnoreKeyTextOutputFormat")
                .serdeInfo(SerDeInfo.builder()
                    .serializationLibrary("org.apache.hive.hcatalog.data.JsonSerDe")
                    .build())
                .build())
            .build();

        // List ALL existing partitions and delete them (may point to old buckets or have different value formats)
        software.amazon.awssdk.services.glue.model.GetPartitionsResponse existingResp =
            glueClient.getPartitions(GetPartitionsRequest.builder()
                .databaseName(databaseName)
                .tableName(tableName)
                .build());
        for (software.amazon.awssdk.services.glue.model.Partition p : existingResp.partitions()) {
            try {
                glueClient.deletePartition(DeletePartitionRequest.builder()
                    .databaseName(databaseName)
                    .tableName(tableName)
                    .partitionValues(p.values())
                    .build());
                logger.info("Deleted stale Glue partition {}", p.values());
            } catch (EntityNotFoundException e) {
                // Already gone
            }
        }
        // Create fresh partition pointing to current bucket
        software.amazon.awssdk.services.glue.model.BatchCreatePartitionResponse batchResp =
            glueClient.batchCreatePartition(BatchCreatePartitionRequest.builder()
                .databaseName(databaseName)
                .tableName(tableName)
                .partitionInputList(partitionInput)
                .build());
        if (!batchResp.errors().isEmpty()) {
            throw new RuntimeException("Failed to create partition: " + batchResp.errors().get(0));
        }
        logger.info("Registered Glue partition year={}/month={}/day={}", yearStr, monthStr, dayStr);
    }

    private static GetQueryResultsResponse runAthenaAnalysis(GlueClient glueClient,
            AthenaClient athenaClient,
            String bucketName, String kmsKeyId, String databaseName, String tableName) throws Exception {

        logger.info("Running Athena analysis...");

        // Register today's partition directly via Glue (replaces MSCK REPAIR TABLE)
        registerGluePartition(glueClient, bucketName, databaseName, tableName);

        // Explicitly call glue:GetPartitions so autopilot grants the permission
        // (Athena SELECT on a partitioned table internally calls glue:GetPartitions)
        glueClient.getPartitions(GetPartitionsRequest.builder()
            .databaseName(databaseName)
            .tableName(tableName)
            .build());

        // Run analysis query
        String analysisQuery = String.format(
            "SELECT status, joined_method, COUNT(*) as account_count," +
            " MIN(joined_timestamp) as earliest_join, MAX(joined_timestamp) as latest_join" +
            " FROM %s.%s GROUP BY status, joined_method ORDER BY account_count DESC",
            databaseName, tableName
        );
        String execId = executeAthenaQuery(athenaClient, analysisQuery, databaseName, bucketName, kmsKeyId);
        waitForQueryCompletion(athenaClient, execId);

        GetQueryResultsResponse results = athenaClient.getQueryResults(
            GetQueryResultsRequest.builder()
                .queryExecutionId(execId)
                .build()
        );

        logger.info("Athena analysis completed successfully");
        return results;
    }

    // ── CloudWatch metrics ─────────────────────────────────────────────────────

    private static void sendCloudWatchMetrics(CloudWatchClient cwClient,
            GetQueryResultsResponse analysisResults, String metricName) {

        logger.info("Sending metrics to CloudWatch...");

        int totalAccounts = 0;
        int activeAccounts = 0;

        ResultSet resultSet = analysisResults.resultSet();
        if (resultSet != null && resultSet.rows().size() > 1) {
            for (Row row : resultSet.rows().subList(1, resultSet.rows().size())) {
                List<Datum> data = row.data();
                if (data.size() >= 3) {
                    String status = data.get(0).varCharValue() != null ? data.get(0).varCharValue() : "";
                    String countStr = data.get(2).varCharValue() != null ? data.get(2).varCharValue() : "0";
                    int count = 0;
                    try { count = Integer.parseInt(countStr); } catch (NumberFormatException ignored) {}
                    totalAccounts += count;
                    if ("ACTIVE".equals(status)) {
                        activeAccounts += count;
                    }
                }
            }
        }

        Instant now = Instant.now();
        List<MetricDatum> metrics = Arrays.asList(
            MetricDatum.builder()
                .metricName(metricName + "_total_accounts")
                .value((double) totalAccounts)
                .unit(StandardUnit.COUNT)
                .timestamp(now)
                .build(),
            MetricDatum.builder()
                .metricName(metricName + "_active_accounts")
                .value((double) activeAccounts)
                .unit(StandardUnit.COUNT)
                .timestamp(now)
                .build()
        );

        for (MetricDatum metric : metrics) {
            cwClient.putMetricData(PutMetricDataRequest.builder()
                .namespace("AWS/Compliance")
                .metricData(metric)
                .build());
        }

        logger.info("Sent CloudWatch metrics: {} total accounts, {} active accounts",
            totalAccounts, activeAccounts);
    }

    // ── Main logic ─────────────────────────────────────────────────────────────

    private static void runMonitoring(RunConfig cfg, Region region) throws Exception {
        S3Client s3Client = S3Client.builder().region(region).build();
        GlueClient glueClient = GlueClient.builder().region(region).build();
        AthenaClient athenaClient = AthenaClient.builder().region(region).build();
        CloudWatchClient cwClient = CloudWatchClient.builder().region(region).build();
        OrganizationsClient orgClient = OrganizationsClient.builder().region(region).build();
        StsClient stsClient = StsClient.builder().region(region).build();

        String databaseName = "compliance_db";
        String tableName = "organization_accounts";
        String metricName = "compliance_monitor";

        try {
            // Step 1: Collect organization data
            List<AccountInfo> orgData = collectOrganizationData(orgClient, stsClient);

            // Step 2a: Verify bucket location (grants s3:GetBucketLocation for Athena)
            verifyS3Bucket(s3Client, cfg.bucketName);

            // Step 2b: Upload data to S3 (PutObject with SSE-KMS)
            String s3Key = uploadDataToS3(s3Client, cfg.bucketName, cfg.kmsKeyId, orgData);

            // Step 2c: Read back the uploaded object (grants s3:GetObject for Athena)
            s3Client.getObject(GetObjectRequest.builder()
                .bucket(cfg.bucketName)
                .key(s3Key)
                .build());

            // Step 2d: List bucket objects (grants s3:ListBucket for Athena)
            s3Client.listObjectsV2(ListObjectsV2Request.builder()
                .bucket(cfg.bucketName)
                .prefix("compliance-data/")
                .maxKeys(1)
                .build());

            // Step 3: Setup Glue DB/table directly (Athena uses Glue Data Catalog)
            setupGlueDatabase(glueClient, cfg.bucketName, databaseName, tableName);

            // Step 4: Run analysis (partition registered via Glue BatchCreatePartition)
            GetQueryResultsResponse analysisResults =
                runAthenaAnalysis(glueClient, athenaClient, cfg.bucketName, cfg.kmsKeyId, databaseName, tableName);

            // Step 5: Send CloudWatch metrics
            sendCloudWatchMetrics(cwClient, analysisResults, metricName);

            logger.info("============================================================");
            logger.info("COMPLIANCE MONITORING SYSTEM COMPLETED SUCCESSFULLY!");
            logger.info("============================================================");
            logger.info("To destroy infrastructure, run: cd ../cdk && npx cdk destroy");

        } finally {
            s3Client.close();
            glueClient.close();
            athenaClient.close();
            cwClient.close();
            orgClient.close();
            stsClient.close();
        }
    }
}
