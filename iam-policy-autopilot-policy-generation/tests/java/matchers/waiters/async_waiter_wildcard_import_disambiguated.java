// Wildcard import only — no specific import for CloudVaultAsyncWaiter.
// The disambiguator must derive the service from the serviceId metadata
// ("CloudVault" → "CloudVaultAsyncWaiter") rather than falling back to the
// broad import filter.
import software.amazon.awssdk.services.cloudvault.*;

class AsyncWaiterWildcardImportDisambiguated {
    void run() {
        CloudVaultAsyncWaiter waiter = cloudVaultAsyncClient.waiter();
        waiter.waitUntilResourceReady(request);
    }
}
