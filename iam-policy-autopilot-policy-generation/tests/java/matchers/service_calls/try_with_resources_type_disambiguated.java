import software.amazon.awssdk.services.s3.S3Client;
import software.amazon.awssdk.services.s3control.S3ControlClient;

class TryWithResourcesTypeDisambiguated {
    void run(PutObjectRequest request) {
        try (S3Client s3 = S3Client.create();
             S3ControlClient s3control = S3ControlClient.create()) {
            s3.putObject(request);
            s3control.putObject(request);
        }
    }
}
