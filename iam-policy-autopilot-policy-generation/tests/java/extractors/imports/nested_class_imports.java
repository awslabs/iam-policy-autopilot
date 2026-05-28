import software.amazon.awssdk.services.s3.model.PutObjectRequest;
import software.amazon.awssdk.services.s3.model.GetObjectResponse;
import software.amazon.awssdk.services.dynamodb.model.AttributeValue;

public class MyClass {
    public void doSomething() {
        PutObjectRequest request = PutObjectRequest.builder().build();
    }
}
