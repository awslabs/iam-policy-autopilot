import software.amazon.awssdk.services.s3.S3Client;

class StaticInitializerLocalVar {
    static {
        S3Client s3 = S3Client.create();
        s3.listBuckets();
    }
}
