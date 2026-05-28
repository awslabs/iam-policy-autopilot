import software.amazon.awssdk.services.s3.S3Client;

class StaticFieldCall {
    static S3Client s3 = S3Client.create();

    static void run() {
        s3.listBuckets();
    }
}
