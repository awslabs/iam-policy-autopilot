import software.amazon.awssdk.services.s3.S3Client;
import software.amazon.awssdk.services.s3control.S3ControlClient;

class InstanceofPatternBindingTwoServices {
    void run(Object client, PutObjectRequest request) {
        if (client instanceof S3Client s3) {
            s3.putObject(request);
        }
        if (client instanceof S3ControlClient s3control) {
            s3control.putObject(request);
        }
    }
}
