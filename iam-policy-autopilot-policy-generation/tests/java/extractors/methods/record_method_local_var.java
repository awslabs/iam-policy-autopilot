import software.amazon.awssdk.services.s3.S3Client;
import software.amazon.awssdk.services.dynamodb.DynamoDbClient;
import software.amazon.awssdk.services.s3.model.PutObjectRequest;

record AwsService(DynamoDbClient s3) {
    void upload(PutObjectRequest req) {
        S3Client s3 = S3Client.create();
        s3.putObject(req);
    }
}
