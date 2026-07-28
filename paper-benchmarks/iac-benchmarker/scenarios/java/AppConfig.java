package com.example.auditlog;

import software.amazon.awssdk.services.ssm.SsmClient;
import software.amazon.awssdk.services.ssm.model.GetParameterRequest;
import software.amazon.awssdk.services.ssm.model.ParameterNotFoundException;

import java.util.Optional;
import java.util.logging.Logger;

/**
 * Loads runtime configuration from environment variables, with optional
 * fallback to AWS SSM Parameter Store for sensitive values.
 */
public class AppConfig {

    private static final Logger LOG = Logger.getLogger(AppConfig.class.getName());

    // SQS
    public final String auditQueueUrl;
    public final int maxMessages;
    public final int visibilityTimeoutSeconds;
    public final int waitTimeSeconds;

    // DynamoDB
    public final String userProfilesTable;
    public final String dynamoRegion;

    // S3
    public final String archiveBucket;
    public final String archivePrefix;

    // KMS
    public final String kmsKeyId;

    // SNS
    public final String complianceTopicArn;

    // CloudWatch Logs
    public final String logGroupName;
    public final String logStreamPrefix;

    // SSM
    public final String ssmPrefix;

    private AppConfig(Builder b) {
        this.auditQueueUrl           = b.auditQueueUrl;
        this.maxMessages             = b.maxMessages;
        this.visibilityTimeoutSeconds = b.visibilityTimeoutSeconds;
        this.waitTimeSeconds         = b.waitTimeSeconds;
        this.userProfilesTable       = b.userProfilesTable;
        this.dynamoRegion            = b.dynamoRegion;
        this.archiveBucket           = b.archiveBucket;
        this.archivePrefix           = b.archivePrefix;
        this.kmsKeyId                = b.kmsKeyId;
        this.complianceTopicArn      = b.complianceTopicArn;
        this.logGroupName            = b.logGroupName;
        this.logStreamPrefix         = b.logStreamPrefix;
        this.ssmPrefix               = b.ssmPrefix;
    }

    /** Load configuration from environment variables. */
    public static AppConfig fromEnvironment() {
        return new Builder()
            .auditQueueUrl(requireEnv("AUDIT_QUEUE_URL"))
            .maxMessages(parseInt("MAX_MESSAGES", 10))
            .visibilityTimeoutSeconds(parseInt("VISIBILITY_TIMEOUT_SECONDS", 300))
            .waitTimeSeconds(parseInt("WAIT_TIME_SECONDS", 20))
            .userProfilesTable(getEnv("USER_PROFILES_TABLE", "user-profiles"))
            .dynamoRegion(getEnv("AWS_REGION", "us-east-1"))
            .archiveBucket(requireEnv("ARCHIVE_BUCKET"))
            .archivePrefix(getEnv("ARCHIVE_PREFIX", "audit/"))
            .kmsKeyId(requireEnv("KMS_KEY_ID"))
            .complianceTopicArn(requireEnv("COMPLIANCE_TOPIC_ARN"))
            .logGroupName(getEnv("LOG_GROUP_NAME", "/audit-log-archiver"))
            .logStreamPrefix(getEnv("LOG_STREAM_PREFIX", "processor"))
            .ssmPrefix(getEnv("SSM_PREFIX", "/audit-log-archiver"))
            .build();
    }

    /**
     * Optionally load a secret from SSM Parameter Store.
     * Returns empty if the parameter does not exist or SSM is unavailable.
     */
    public Optional<String> loadSsmSecret(SsmClient ssm, String paramName) {
        String fullPath = ssmPrefix + "/" + paramName;
        try {
            var resp = ssm.getParameter(GetParameterRequest.builder()
                .name(fullPath)
                .withDecryption(true)
                .build());
            LOG.info("Loaded SSM parameter: " + fullPath);
            return Optional.of(resp.parameter().value());
        } catch (ParameterNotFoundException e) {
            LOG.fine("SSM parameter not found: " + fullPath);
            return Optional.empty();
        } catch (Exception e) {
            LOG.warning("Could not load SSM parameter " + fullPath + ": " + e.getMessage());
            return Optional.empty();
        }
    }

    // ── helpers ──────────────────────────────────────────────────────────────

    private static String requireEnv(String name) {
        String v = System.getenv(name);
        if (v == null || v.isBlank()) {
            throw new IllegalStateException("Required environment variable '" + name + "' is not set.");
        }
        return v;
    }

    private static String getEnv(String name, String fallback) {
        String v = System.getenv(name);
        return (v != null && !v.isBlank()) ? v : fallback;
    }

    private static int parseInt(String name, int fallback) {
        try {
            String v = System.getenv(name);
            return (v != null) ? Integer.parseInt(v) : fallback;
        } catch (NumberFormatException e) {
            return fallback;
        }
    }

    // ── Builder ───────────────────────────────────────────────────────────────

    public static class Builder {
        String auditQueueUrl, userProfilesTable, dynamoRegion, archiveBucket,
               archivePrefix, kmsKeyId, complianceTopicArn, logGroupName,
               logStreamPrefix, ssmPrefix;
        int maxMessages, visibilityTimeoutSeconds, waitTimeSeconds;

        public Builder auditQueueUrl(String v)            { auditQueueUrl = v; return this; }
        public Builder maxMessages(int v)                 { maxMessages = v; return this; }
        public Builder visibilityTimeoutSeconds(int v)    { visibilityTimeoutSeconds = v; return this; }
        public Builder waitTimeSeconds(int v)             { waitTimeSeconds = v; return this; }
        public Builder userProfilesTable(String v)        { userProfilesTable = v; return this; }
        public Builder dynamoRegion(String v)             { dynamoRegion = v; return this; }
        public Builder archiveBucket(String v)            { archiveBucket = v; return this; }
        public Builder archivePrefix(String v)            { archivePrefix = v; return this; }
        public Builder kmsKeyId(String v)                 { kmsKeyId = v; return this; }
        public Builder complianceTopicArn(String v)       { complianceTopicArn = v; return this; }
        public Builder logGroupName(String v)             { logGroupName = v; return this; }
        public Builder logStreamPrefix(String v)          { logStreamPrefix = v; return this; }
        public Builder ssmPrefix(String v)                { ssmPrefix = v; return this; }
        public AppConfig build()                          { return new AppConfig(this); }
    }
}
