import software.amazon.awssdk.services.s3.S3Client;

record AwsService(S3Client s3) {
    AwsService {
        s3.listBuckets();
    }
}
