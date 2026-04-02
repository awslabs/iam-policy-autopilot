import software.amazon.awssdk.services.s3.S3AsyncClient;
import software.amazon.awssdk.services.dynamodb.DynamoDbAsyncClient;

class InferredParamsShadowsField {
    private final S3AsyncClient s3 = S3AsyncClient.create();

    void run(java.util.Map<DynamoDbAsyncClient, String> clientMap) {
        clientMap.forEach((s3, tableName) -> s3.listTables());
    }
}
