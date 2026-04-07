import software.amazon.awssdk.services.s3.S3Client;
import software.amazon.awssdk.services.dynamodb.DynamoDbClient;

class AnonymousClassOuterScope {
    S3Client s3 = S3Client.create();

    void doWork() {
        DynamoDbClient dynamo = DynamoDbClient.create();

        executor.submit(new Runnable() {
            @Override
            public void run() {
                s3.putObject(null);
                dynamo.listTables();
            }
        });
    }
}
