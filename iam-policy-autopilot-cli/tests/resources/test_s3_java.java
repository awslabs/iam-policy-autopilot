import software.amazon.awssdk.services.s3.S3Client;

class Test {
    void run(S3Client s3Client) {
        s3Client.listBuckets();
        s3Client.putObject(request);
        s3Client.listObjectsV2Paginator(request);
    }
}
