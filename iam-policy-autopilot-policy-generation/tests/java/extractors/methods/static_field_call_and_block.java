import software.amazon.awssdk.services.s3.S3Client;

class StaticFieldCallAndBlock {
    static S3Client s3;

    static {
        s3 = S3Client.create();
    }

    static void run() {
        s3.listBuckets();
    }
}
