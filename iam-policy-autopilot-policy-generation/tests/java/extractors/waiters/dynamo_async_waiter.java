import software.amazon.awssdk.services.dynamodb.DynamoDbAsyncClient;
import software.amazon.awssdk.services.dynamodb.waiters.DynamoDbAsyncWaiter;
import software.amazon.awssdk.core.waiters.WaiterResponse;
import software.amazon.awssdk.services.dynamodb.model.DescribeTableResponse;
import java.util.concurrent.CompletableFuture;

class DynamoAsyncWaiter {
    void run() {
        DynamoDbAsyncClient asyncDynamo = DynamoDbAsyncClient.create();
        DynamoDbAsyncWaiter asyncWaiter = asyncDynamo.waiter();

        CompletableFuture<WaiterResponse<DescribeTableResponse>> waiterResponse =
            asyncWaiter.waitUntilTableNotExists(r -> r.tableName("myTable"));
    }
}
