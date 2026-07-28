package com.example.auditlog;

import software.amazon.awssdk.services.sqs.SqsClient;
import software.amazon.awssdk.services.sqs.model.*;

import java.util.List;
import java.util.function.Consumer;
import java.util.logging.Logger;

/**
 * Long-poll SQS consumer that reads audit event messages and dispatches
 * them to a registered handler.  Extends visibility timeout for slow
 * processing and deletes messages on success.
 */
public class SqsEventReader {

    private static final Logger LOG = Logger.getLogger(SqsEventReader.class.getName());

    private final SqsClient sqs;
    private final AppConfig cfg;
    private volatile boolean running = true;

    public SqsEventReader(SqsClient sqs, AppConfig cfg) {
        this.sqs = sqs;
        this.cfg = cfg;
    }

    /**
     * Start the polling loop.  Calls {@code handler} for each message body.
     * Blocks until {@link #stop()} is called.
     */
    public void run(Consumer<String> handler) {
        LOG.info("SQS reader starting — queue: " + cfg.auditQueueUrl);
        while (running) {
            try {
                pollOnce(handler);
            } catch (Exception e) {
                LOG.severe("Unexpected error in poll loop: " + e.getMessage());
                sleep(5_000);
            }
        }
        LOG.info("SQS reader stopped.");
    }

    public void stop() {
        LOG.info("SQS reader shutting down ...");
        running = false;
    }

    // ── internals ─────────────────────────────────────────────────────────────

    private void pollOnce(Consumer<String> handler) {
        var request = ReceiveMessageRequest.builder()
            .queueUrl(cfg.auditQueueUrl)
            .maxNumberOfMessages(cfg.maxMessages)
            .visibilityTimeout(cfg.visibilityTimeoutSeconds)
            .waitTimeSeconds(cfg.waitTimeSeconds)
            .attributeNamesWithStrings("All")
            .messageAttributeNames("All")
            .build();

        List<Message> messages = sqs.receiveMessage(request).messages();
        if (messages.isEmpty()) return;

        LOG.info("Received " + messages.size() + " message(s)");
        for (Message msg : messages) {
            processMessage(msg, handler);
        }
    }

    private void processMessage(Message msg, Consumer<String> handler) {
        String messageId = msg.messageId();
        try {
            handler.accept(msg.body());
            deleteMessage(msg.receiptHandle());
            LOG.info("Message " + messageId + " processed and deleted");
        } catch (Exception e) {
            LOG.warning("Handler failed for message " + messageId + ": " + e.getMessage());
            // Leave in queue for retry / DLQ.
        }
    }

    private void deleteMessage(String receiptHandle) {
        sqs.deleteMessage(DeleteMessageRequest.builder()
            .queueUrl(cfg.auditQueueUrl)
            .receiptHandle(receiptHandle)
            .build());
    }

    /** Extend visibility timeout for a message still being processed. */
    public void extendVisibility(String receiptHandle, int newTimeout) {
        sqs.changeMessageVisibility(ChangeMessageVisibilityRequest.builder()
            .queueUrl(cfg.auditQueueUrl)
            .receiptHandle(receiptHandle)
            .visibilityTimeout(newTimeout)
            .build());
    }

    /** Return the approximate number of messages in the queue. */
    public int getQueueDepth() {
        try {
            var resp = sqs.getQueueAttributes(GetQueueAttributesRequest.builder()
                .queueUrl(cfg.auditQueueUrl)
                .attributeNamesWithStrings("ApproximateNumberOfMessages")
                .build());
            return Integer.parseInt(
                resp.attributesAsStrings().getOrDefault("ApproximateNumberOfMessages", "0"));
        } catch (Exception e) {
            LOG.warning("Could not get queue depth: " + e.getMessage());
            return -1;
        }
    }

    private static void sleep(long ms) {
        try { Thread.sleep(ms); } catch (InterruptedException ie) { Thread.currentThread().interrupt(); }
    }
}
