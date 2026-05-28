import software.amazon.awssdk.services.s3.S3AsyncClient;
import software.amazon.awssdk.services.dynamodb.DynamoDbAsyncClient;

class InferredLambdaParamShadowsField {
    private final S3AsyncClient s3 = S3AsyncClient.create();

    void run(java.util.List<DynamoDbAsyncClient> clients) {
        clients.forEach(s3 -> s3.listTables());
    }
}
