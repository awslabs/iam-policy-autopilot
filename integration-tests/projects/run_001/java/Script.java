import software.amazon.awssdk.regions.Region;
import software.amazon.awssdk.services.sts.StsClient;
import software.amazon.awssdk.services.sts.model.GetCallerIdentityResponse;
import software.amazon.awssdk.services.redshiftdata.RedshiftDataClient;
import software.amazon.awssdk.services.redshiftdata.model.*;

import com.fasterxml.jackson.databind.ObjectMapper;

import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

import java.io.File;
import java.util.concurrent.TimeUnit;

public class Script {
    private static final Logger logger = LoggerFactory.getLogger(Script.class);
    private static final ObjectMapper objectMapper = new ObjectMapper();

    // ── Config loading ─────────────────────────────────────────────────────────

    static class RunConfig {
        public String bucketName;
        public String redshiftClusterIdentifier;
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

            logger.info("Starting AWS Security and Analytics Platform (data-plane)...");
            logger.info("Using Redshift cluster: {}", cfg.redshiftClusterIdentifier);
            logger.info("Using region:           {}", region);

            runSecurityAnalytics(cfg.redshiftClusterIdentifier, region);

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

    private static String executeRedshiftStatement(
            RedshiftDataClient rdClient,
            String clusterID, String database, String dbUser, String sql) {
        ExecuteStatementResponse response = rdClient.executeStatement(
                ExecuteStatementRequest.builder()
                        .clusterIdentifier(clusterID)
                        .database(database)
                        .dbUser(dbUser)
                        .sql(sql)
                        .build());
        String stmtId = response.id();
        logger.info("Redshift Data API ExecuteStatement submitted, id={}", stmtId);
        return stmtId;
    }

    private static String waitForRedshiftStatement(
            RedshiftDataClient rdClient, String stmtId,
            int pollIntervalSec, int maxWaitSec) throws InterruptedException {
        int elapsed = 0;
        while (elapsed < maxWaitSec) {
            DescribeStatementResponse desc = rdClient.describeStatement(
                    DescribeStatementRequest.builder().id(stmtId).build());
            String status = desc.statusAsString();
            logger.info("  Statement {} status: {}", stmtId, status);
            switch (status) {
                case "FINISHED":
                    return status;
                case "FAILED":
                case "ABORTED":
                    String err = desc.error() != null ? desc.error() : "unknown error";
                    logger.warn("  Statement ended with status {}: {}", status, err);
                    return status;
                default:
                    TimeUnit.SECONDS.sleep(pollIntervalSec);
                    elapsed += pollIntervalSec;
            }
        }
        logger.warn("  Statement {} did not finish within {}s", stmtId, maxWaitSec);
        return "TIMEOUT";
    }

    // ── Main logic ────────────────────────────────────────────────────────────

    private static void runSecurityAnalytics(String clusterID, Region region)
            throws Exception {

        StsClient stsClient = StsClient.builder().region(region).build();
        RedshiftDataClient rdClient = RedshiftDataClient.builder().region(region).build();

        try {
            // ── STS: GetCallerIdentity ─────────────────────────────────────────
            logger.info("Getting AWS account information...");
            String accountId = getAwsAccountId(stsClient);
            logger.info("Using AWS Account ID: {}", accountId);

            String database = "securitydb";
            String dbUser   = "adminuser";

            // ── Redshift Data API: 1. CREATE TABLE ────────────────────────────
            logger.info("Executing Redshift statement 1/3: CREATE TABLE security_events...");
            String createSQL =
                "CREATE TABLE IF NOT EXISTS security_events (\n" +
                "    event_id    VARCHAR(64),\n" +
                "    event_type  VARCHAR(64),\n" +
                "    source_ip   VARCHAR(45),\n" +
                "    user_name   VARCHAR(128),\n" +
                "    timestamp   TIMESTAMP,\n" +
                "    severity    VARCHAR(16),\n" +
                "    description VARCHAR(512)\n" +
                ")";

            String stmtId = executeRedshiftStatement(rdClient, clusterID, database, dbUser, createSQL);
            waitForRedshiftStatement(rdClient, stmtId, 2, 60);

            // ── Redshift Data API: 2. INSERT data ─────────────────────────────
            logger.info("Executing Redshift statement 2/3: INSERT security events...");
            String insertSQL =
                "INSERT INTO security_events\n" +
                "    (event_id, event_type, source_ip, user_name, timestamp, severity, description)\n" +
                "VALUES\n" +
                "    ('evt-001', 'LOGIN_FAILURE',        '192.168.1.100', 'user1', GETDATE(), 'HIGH',     'Multiple failed login attempts'),\n" +
                "    ('evt-002', 'DATA_ACCESS',          '10.0.0.50',     'user2', GETDATE(), 'MEDIUM',   'Unusual data access pattern'),\n" +
                "    ('evt-003', 'PRIVILEGE_ESCALATION', '172.16.0.1',    'user3', GETDATE(), 'CRITICAL', 'Unauthorized privilege escalation attempt')";

            stmtId = executeRedshiftStatement(rdClient, clusterID, database, dbUser, insertSQL);
            waitForRedshiftStatement(rdClient, stmtId, 2, 60);

            // ── Redshift Data API: 3. Analytics SELECT ────────────────────────
            logger.info("Executing Redshift statement 3/3: Analytics query on security_events...");
            String analyticsSQL =
                "SELECT\n" +
                "    severity,\n" +
                "    COUNT(*)       AS event_count,\n" +
                "    MIN(timestamp) AS first_seen,\n" +
                "    MAX(timestamp) AS last_seen\n" +
                "FROM security_events\n" +
                "GROUP BY severity\n" +
                "ORDER BY event_count DESC";

            stmtId = executeRedshiftStatement(rdClient, clusterID, database, dbUser, analyticsSQL);
            waitForRedshiftStatement(rdClient, stmtId, 2, 60);

            logger.info("============================================================");
            logger.info("APPLICATION COMPLETED SUCCESSFULLY!");
            logger.info("============================================================");
            logger.info("Resources used (data-plane):");
            logger.info("  - STS:           GetCallerIdentity (account: {})", accountId);
            logger.info("  - Redshift Data: ExecuteStatement x3 (cluster: {})", clusterID);
            logger.info("============================================================");

        } finally {
            stsClient.close();
            rdClient.close();
        }
    }
}
