import software.amazon.awssdk.services.s3.S3Client;
class Test {
    void run(S3Client s3) {
        s3.listBuckets();
    }
}
