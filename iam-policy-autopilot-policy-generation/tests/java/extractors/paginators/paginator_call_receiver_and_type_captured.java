import software.amazon.awssdk.services.cloudvault.CloudVaultClient;

class PaginatorCallReceiverAndTypeCaptured {
    void run(Object cloudVaultClient) {
        CloudVaultClient client = CloudVaultClient.create();
        client.listResourcesPaginator(request);
    }
}
