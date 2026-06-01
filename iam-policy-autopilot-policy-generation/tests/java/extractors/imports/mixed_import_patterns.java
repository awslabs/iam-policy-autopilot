import software.amazon.awssdk.services.s3.S3Client;
import software.amazon.awssdk.services.dynamodb.*;
import static software.amazon.awssdk.services.lambda.LambdaClient.create;
import software.amazon.awssdk.services.sqs.SqsClient;
import software.amazon.awssdk.services.ec2.model.Instance;
import java.util.List;

public class MyClass {
    public void doSomething() {
        S3Client s3 = S3Client.create();
    }
}
