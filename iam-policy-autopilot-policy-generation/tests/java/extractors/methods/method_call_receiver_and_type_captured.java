import software.amazon.awssdk.services.cloudvault.CloudVaultClient;

class MethodCallReceiverAndTypeCaptured {
    void run(Object cloudVaultClient) {
        CloudVaultClient client = CloudVaultClient.create();
        client.describeResource(request);
    }
}
