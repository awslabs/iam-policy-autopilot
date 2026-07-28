package com.example.auditlog;

import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.node.ObjectNode;
import software.amazon.awssdk.core.sync.RequestBody;
import software.amazon.awssdk.services.cloudwatchlogs.CloudWatchLogsClient;
import software.amazon.awssdk.services.cloudwatchlogs.model.*;
import software.amazon.awssdk.services.dynamodb.DynamoDbClient;
import software.amazon.awssdk.services.kms.KmsClient;
import software.amazon.awssdk.services.s3.S3Client;
import software.amazon.awssdk.services.s3.model.PutObjectRequest;
import software.amazon.awssdk.services.sns.SnsClient;
import software.amazon.awssdk.services.sns.model.PublishRequest;
import software.amazon.awssdk.services.sqs.SqsClient;

import java.nio.charset.StandardCharsets;
import java.time.Instant;
import java.util.UUID;
import java.util.logging.Logger;

/**
 * Main orchestrator for the audit log archiving pipeline.
 *
 * <p>For each SQS message:
 * <ol>
 *   <li>Parse the audit event JSON.</li>
 *   <li>Enrich with user profile from DynamoDB.</li>
 *   <li>Sign the enriched record with KMS.</li>
 *   <li>Archive the signed record to S3.</li>
 *   <li>Publish a compliance summary to SNS (on anomaly).</li>
 *   <li>Write a structured log entry to CloudWatch Logs.</li>
 * </ol>
 */
public class AuditLogProcessor {

    private static final Logger LOG = Logger.getLogger(AuditLogProcessor.class.getName());
    private static final ObjectMapper MAPPER = new ObjectMapper();

    private final AppConfig cfg;
    private final SqsEventReader reader;
    private final UserProfileEnricher enricher;
    private final KmsRecordSigner signer;
    private final S3Client s3;
    private final SnsClient sns;
    private final CloudWatchLogsClient cwLogs;
    private String cwLogStreamName;

    public AuditLogProcessor(AppConfig cfg) {
        this.cfg = cfg;

        var sqsClient    = SqsClient.create();
        var dynamoClient = DynamoDbClient.create();
        var kmsClient    = KmsClient.create();
        this.s3          = S3Client.create();
        this.sns         = SnsClient.create();
        this.cwLogs      = CloudWatchLogsClient.create();

        this.reader   = new SqsEventReader(sqsClient, cfg);
        this.enricher = new UserProfileEnricher(dynamoClient, cfg);
        this.signer   = new KmsRecordSigner(kmsClient, cfg);
    }

    /** Start the processor — blocks until interrupted. */
    public void run() {
        signer.validateKey();
        ensureLogStream();

        Runtime.getRuntime().addShutdownHook(new Thread(reader::stop));

        LOG.info("Audit log processor starting");
        reader.run(this::handleMessage);
    }

    // ── message handler ───────────────────────────────────────────────────────

    private void handleMessage(String body) {
        try {
            ObjectNode event = (ObjectNode) MAPPER.readTree(body);
            String userId    = event.path("user_id").asText("unknown");
            String action    = event.path("action").asText("unknown");
            String recordId  = UUID.randomUUID().toString();

            // 1. Enrich with user profile.
            enricher.getProfile(userId).ifPresent(profile -> {
                event.put("user_email",      profile.email());
                event.put("user_department", profile.department());
                event.put("user_role",       profile.role());
                event.put("account_status",  profile.accountStatus());
            });

            // 2. Sign the enriched record.
            String enrichedJson = MAPPER.writeValueAsString(event);
            String signature    = signer.sign(enrichedJson);

            ObjectNode envelope = MAPPER.createObjectNode();
            envelope.put("record_id",  recordId);
            envelope.put("timestamp",  Instant.now().toString());
            envelope.set("payload",    event);
            envelope.put("signature",  signature);
            envelope.put("key_id",     cfg.kmsKeyId);

            String envelopeJson = MAPPER.writeValueAsString(envelope);

            // 3. Archive to S3.
            String s3Key = cfg.archivePrefix + Instant.now().toString().substring(0, 10)
                + "/" + recordId + ".json";
            s3.putObject(
                PutObjectRequest.builder()
                    .bucket(cfg.archiveBucket)
                    .key(s3Key)
                    .contentType("application/json")
                    .build(),
                RequestBody.fromBytes(envelopeJson.getBytes(StandardCharsets.UTF_8))
            );
            LOG.info("Archived record " + recordId + " → s3://" + cfg.archiveBucket + "/" + s3Key);

            // 4. Publish compliance alert for suspicious actions.
            if (isSuspicious(action)) {
                sns.publish(PublishRequest.builder()
                    .topicArn(cfg.complianceTopicArn)
                    .subject("Suspicious audit event: " + action)
                    .message(envelopeJson)
                    .build());
                LOG.warning("Published compliance alert for action: " + action);
            }

            // 5. Write structured log to CloudWatch.
            putLogEvent(envelopeJson);

        } catch (Exception e) {
            LOG.severe("Failed to process audit event: " + e.getMessage());
            throw new RuntimeException(e);
        }
    }

    // ── CloudWatch Logs helpers ───────────────────────────────────────────────

    private void ensureLogStream() {
        cwLogStreamName = cfg.logStreamPrefix + "-" + Instant.now().toString().substring(0, 10);
        try {
            cwLogs.createLogStream(CreateLogStreamRequest.builder()
                .logGroupName(cfg.logGroupName)
                .logStreamName(cwLogStreamName)
                .build());
        } catch (ResourceAlreadyExistsException ignored) {
            // Stream already exists — fine.
        }
        LOG.info("CloudWatch log stream: " + cwLogStreamName);
    }

    private void putLogEvent(String message) {
        cwLogs.putLogEvents(PutLogEventsRequest.builder()
            .logGroupName(cfg.logGroupName)
            .logStreamName(cwLogStreamName)
            .logEvents(InputLogEvent.builder()
                .timestamp(Instant.now().toEpochMilli())
                .message(message)
                .build())
            .build());
    }

    // ── helpers ──────────────────────────────────────────────────────────────

    private static boolean isSuspicious(String action) {
        return action.startsWith("DELETE") || action.startsWith("ADMIN") || action.contains("PRIVILEGE");
    }

    // ── entry point ───────────────────────────────────────────────────────────

    public static void main(String[] args) {
        AppConfig cfg = AppConfig.fromEnvironment();
        new AuditLogProcessor(cfg).run();
    }
}
