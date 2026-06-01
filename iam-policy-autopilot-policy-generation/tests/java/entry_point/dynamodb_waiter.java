import software.amazon.awssdk.services.dynamodb.DynamoDbClient;
import software.amazon.awssdk.services.dynamodb.waiters.DynamoDbWaiter;

class DynamoDbWaiterExample {
    void run(DynamoDbClient dynamoDbClient) {
        DynamoDbWaiter waiter = dynamoDbClient.waiter();
        waiter.waitUntilTableExists(r -> r.tableName("myTable"));
    }
}
