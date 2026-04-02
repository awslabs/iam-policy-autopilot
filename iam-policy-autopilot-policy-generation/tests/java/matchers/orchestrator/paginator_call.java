import software.amazon.awssdk.services.s3.S3Client;
class Test {
    void run() {
        ListObjectsV2Iterable pages = s3Client.listObjectsV2Paginator(request);
    }
}
