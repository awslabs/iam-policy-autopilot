import software.amazon.awssdk.services.cloudvault.waiters.CloudVaultWaiter;
import software.amazon.awssdk.services.datastore.waiters.DataStoreWaiter;

class WaiterParameterDisambiguator {
    void waitForCloudVault(CloudVaultWaiter waiter) {
        waiter.waitUntilResourceReady(request);
    }

    void waitForDataStore(DataStoreWaiter waiter) {
        waiter.waitUntilResourceReady(request);
    }
}
