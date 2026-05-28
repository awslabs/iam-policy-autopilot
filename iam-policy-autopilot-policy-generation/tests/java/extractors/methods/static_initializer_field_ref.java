import software.amazon.awssdk.services.s3.S3Client;

class StaticInitializerFieldRef {
    static S3Client s3 = S3Client.create();

    static {
        s3.listBuckets();
    }
}
