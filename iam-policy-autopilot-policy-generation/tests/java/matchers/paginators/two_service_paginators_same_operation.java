import software.amazon.awssdk.services.cloudvault.CloudVaultClient;
import software.amazon.awssdk.services.datastore.DataStoreClient;

class TwoServicePaginatorsDisambiguator {
    void run(CloudVaultClient cloudVaultClient, DataStoreClient dataStoreClient) {
        cloudVaultClient.listResourcesPaginator(request);
        dataStoreClient.listResourcesPaginator(request);
    }
}
