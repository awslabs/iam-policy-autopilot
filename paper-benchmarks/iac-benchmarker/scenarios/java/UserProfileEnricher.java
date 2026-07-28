package com.example.auditlog;

import software.amazon.awssdk.services.dynamodb.DynamoDbClient;
import software.amazon.awssdk.services.dynamodb.model.*;

import java.util.Map;
import java.util.Optional;
import java.util.logging.Logger;

/**
 * Enriches raw audit log events with user profile metadata from DynamoDB.
 * The {@code user-profiles} table uses {@code user_id} (String) as the
 * partition key.
 */
public class UserProfileEnricher {

    private static final Logger LOG = Logger.getLogger(UserProfileEnricher.class.getName());

    private final DynamoDbClient dynamo;
    private final String tableName;

    public UserProfileEnricher(DynamoDbClient dynamo, AppConfig cfg) {
        this.dynamo    = dynamo;
        this.tableName = cfg.userProfilesTable;
    }

    /**
     * Look up a user profile by {@code userId}.
     *
     * @return an Optional containing the item attributes, or empty if not found.
     */
    public Optional<UserProfile> getProfile(String userId) {
        try {
            var resp = dynamo.getItem(GetItemRequest.builder()
                .tableName(tableName)
                .key(Map.of("user_id", AttributeValue.fromS(userId)))
                .build());

            if (!resp.hasItem() || resp.item().isEmpty()) {
                LOG.fine("No profile found for user: " + userId);
                return Optional.empty();
            }

            Map<String, AttributeValue> item = resp.item();
            return Optional.of(new UserProfile(
                userId,
                getString(item, "email"),
                getString(item, "department"),
                getString(item, "role"),
                getString(item, "account_status")
            ));
        } catch (ResourceNotFoundException e) {
            LOG.warning("DynamoDB table not found: " + tableName);
            return Optional.empty();
        } catch (Exception e) {
            LOG.warning("DynamoDB lookup failed for user " + userId + ": " + e.getMessage());
            return Optional.empty();
        }
    }

    /**
     * Write or update a user profile record.
     */
    public void putProfile(UserProfile profile) {
        dynamo.putItem(PutItemRequest.builder()
            .tableName(tableName)
            .item(Map.of(
                "user_id",        AttributeValue.fromS(profile.userId()),
                "email",          AttributeValue.fromS(profile.email()),
                "department",     AttributeValue.fromS(profile.department()),
                "role",           AttributeValue.fromS(profile.role()),
                "account_status", AttributeValue.fromS(profile.accountStatus())
            ))
            .build());
        LOG.info("Stored profile for user: " + profile.userId());
    }

    /**
     * Update the account status of an existing user profile.
     */
    public void updateAccountStatus(String userId, String newStatus) {
        dynamo.updateItem(UpdateItemRequest.builder()
            .tableName(tableName)
            .key(Map.of("user_id", AttributeValue.fromS(userId)))
            .updateExpression("SET account_status = :s")
            .expressionAttributeValues(Map.of(":s", AttributeValue.fromS(newStatus)))
            .build());
        LOG.info("Updated account_status for user " + userId + " → " + newStatus);
    }

    // ── helpers ──────────────────────────────────────────────────────────────

    private static String getString(Map<String, AttributeValue> item, String key) {
        AttributeValue v = item.get(key);
        return (v != null && v.s() != null) ? v.s() : "";
    }

    // ── value record ─────────────────────────────────────────────────────────

    public record UserProfile(
        String userId,
        String email,
        String department,
        String role,
        String accountStatus
    ) {}
}
