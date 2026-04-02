// Wildcard import only — no specific import for CloudVaultClient.
// The disambiguator must derive "CloudVaultClient" from serviceId "CloudVault" + "Client" suffix.
import software.amazon.awssdk.services.cloudvault.*;

class PaginatorWildcardImportDisambiguated {
    void run(CloudVaultClient client) {
        client.listResourcesPaginator(request);
    }
}
