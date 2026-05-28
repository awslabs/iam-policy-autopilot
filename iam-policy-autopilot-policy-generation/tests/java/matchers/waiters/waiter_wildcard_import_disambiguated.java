// Wildcard import only — no specific import for CloudVaultWaiter.
// The disambiguator must derive the service from the serviceId metadata
// ("CloudVault" → "CloudVaultWaiter") rather than falling back to the
// broad import filter.
import software.amazon.awssdk.services.cloudvault.*;

class WaiterWildcardImportDisambiguated {
    void run() {
        CloudVaultWaiter waiter = cloudVaultClient.waiter();
        waiter.waitUntilResourceReady(request);
    }
}
