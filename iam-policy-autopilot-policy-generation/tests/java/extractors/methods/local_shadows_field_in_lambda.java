import software.amazon.awssdk.services.s3.S3AsyncClient;
import software.amazon.awssdk.services.dynamodb.DynamoDbAsyncClient;

class LocalShadowsFieldInLambda {
    private final S3AsyncClient s3 = S3AsyncClient.create();

    void run() {
        DynamoDbAsyncClient s3 = DynamoDbAsyncClient.create();
        s3.listTables()
            .thenCompose(resp -> s3.describeTable(r -> r.tableName("my-table")));
    }
}
