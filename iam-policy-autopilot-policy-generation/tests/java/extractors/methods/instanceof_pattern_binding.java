import software.amazon.awssdk.services.s3.S3Client;

class InstanceofPatternBinding {
    void run(Object client, PutObjectRequest request) {
        if (client instanceof S3Client s3) {
            s3.putObject(request);
        }
    }
}
