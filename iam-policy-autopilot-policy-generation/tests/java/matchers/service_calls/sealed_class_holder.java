import software.amazon.awssdk.services.s3.S3Client;

sealed interface AwsClientHolder permits S3Holder {
    void execute(PutObjectRequest request);
}

final class S3Holder implements AwsClientHolder {
    private final S3Client s3;

    S3Holder(S3Client s3) {
        this.s3 = s3;
    }

    @Override
    public void execute(PutObjectRequest request) {
        s3.putObject(request);
    }
}
