import software.amazon.awssdk.services.cloudvault.CloudVaultClient;
import software.amazon.awssdk.services.cloudvault.waiters.CloudVaultWaiter;
import software.amazon.awssdk.services.datastore.DataStoreClient;
import software.amazon.awssdk.services.datastore.waiters.DataStoreWaiter;

class TwoServiceWaiterClash {
    void run(CloudVaultClient cloudVaultClient, DataStoreClient dataStoreClient) {
        CloudVaultWaiter cloudVaultWaiter = cloudVaultClient.waiter();
        DataStoreWaiter dataStoreWaiter = dataStoreClient.waiter();

        cloudVaultWaiter.waitUntilResourceReady(request);
        dataStoreWaiter.waitUntilResourceReady(request);
    }
}
