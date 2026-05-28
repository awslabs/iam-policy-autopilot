import software.amazon.awssdk.services.dynamodb.DynamoDbClient;
import software.amazon.awssdk.services.dynamodb.waiters.DynamoDbWaiter;
import software.amazon.awssdk.core.waiters.WaiterResponse;
import software.amazon.awssdk.services.dynamodb.model.DescribeTableResponse;

class DynamoSyncWaiter {
    void run() {
        DynamoDbClient dynamo = DynamoDbClient.create();
        DynamoDbWaiter waiter = dynamo.waiter();

        WaiterResponse<DescribeTableResponse> waiterResponse =
            waiter.waitUntilTableExists(r -> r.tableName("myTable"));
    }
}
