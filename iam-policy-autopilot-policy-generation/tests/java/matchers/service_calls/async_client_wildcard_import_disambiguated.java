// Wildcard import only — no specific import for CloudVaultAsyncClient.
// The disambiguator must derive "CloudVaultAsyncClient" from serviceId "CloudVault" + "AsyncClient" suffix.
import software.amazon.awssdk.services.cloudvault.*;

class AsyncClientWildcardImportDisambiguated {
    void run(CloudVaultAsyncClient client) {
        client.describeResource(request);
    }
}
