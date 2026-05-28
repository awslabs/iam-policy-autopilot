import software.amazon.awssdk.services.s3.S3Client;

class FieldWithoutInitializer {
    private final S3Client s3Client;

    FieldWithoutInitializer(S3Client s3Client) {
        this.s3Client = s3Client;
    }

    void run() {
        s3Client.listBuckets();
    }
}
