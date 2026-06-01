import software.amazon.awssdk.services.cloudvault.CloudVaultClient;
import software.amazon.awssdk.services.datastore.DataStoreClient;

class TwoServiceCallsDisambiguator {
    void run(CloudVaultClient cloudVaultClient, DataStoreClient dataStoreClient) {
        cloudVaultClient.describeResource(request);
        dataStoreClient.describeResource(request);
    }
}
