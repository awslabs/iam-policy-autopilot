import software.amazon.awssdk.services.dynamodb.DynamoDbAsyncClient;
import software.amazon.awssdk.services.dynamodb.model.ListTablesRequest;
import software.amazon.awssdk.services.dynamodb.model.ListTablesResponse;
import software.amazon.awssdk.services.dynamodb.paginators.ListTablesPublisher;
import java.util.concurrent.CompletableFuture;

class DynamoAsyncPaginator {
    void run() {
        DynamoDbAsyncClient asyncClient = DynamoDbAsyncClient.create();
        ListTablesRequest listTablesRequest = ListTablesRequest.builder().limit(3).build();
        ListTablesPublisher publisher = asyncClient.listTablesPaginator(listTablesRequest);

        CompletableFuture<Void> future = publisher.subscribe(
            response -> response.tableNames().forEach(System.out::println));
    }
}
