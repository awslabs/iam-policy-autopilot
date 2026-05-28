import software.amazon.awssdk.services.s3.S3Client;

class Test {
    void run(S3Client s3) {
        ListObjectsV2Iterable pages = s3.listObjectsV2Paginator(request);
    }
}
