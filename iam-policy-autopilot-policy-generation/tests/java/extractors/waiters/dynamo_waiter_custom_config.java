import software.amazon.awssdk.services.dynamodb.DynamoDbClient;
import software.amazon.awssdk.services.dynamodb.waiters.DynamoDbWaiter;

class DynamoWaiterCustomConfig {
    void run(DynamoDbClient dynamoDbClient) {
        DynamoDbWaiter waiter = DynamoDbWaiter.builder()
            .overrideConfiguration(b -> b.maxAttempts(10))
            .client(dynamoDbClient)
            .build();

        waiter.waitUntilTableNotExists(b -> b.tableName("myTable"),
            o -> o.maxAttempts(10));
    }
}
