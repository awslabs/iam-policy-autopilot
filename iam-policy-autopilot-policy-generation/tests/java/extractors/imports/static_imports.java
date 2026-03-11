import static software.amazon.awssdk.services.s3.S3Client.create;
import static software.amazon.awssdk.services.dynamodb.DynamoDbClient.builder;
import static java.util.Collections.emptyList;

public class MyClass {
    public void doSomething() {
        S3Client s3 = create();
    }
}
