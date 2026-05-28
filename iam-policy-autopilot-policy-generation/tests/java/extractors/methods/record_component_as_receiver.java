import software.amazon.awssdk.services.s3.S3Client;
import software.amazon.awssdk.services.s3.model.PutObjectRequest;

record AwsService(S3Client s3) {
    void upload(PutObjectRequest req) {
        s3.putObject(req);
    }
}
