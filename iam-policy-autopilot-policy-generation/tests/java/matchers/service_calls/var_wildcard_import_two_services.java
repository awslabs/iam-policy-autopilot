// NOTE: This file does not compile — `getClient()` and `request` are undeclared.
// It is a syntactic test fixture only; the extractor works on source text via tree-sitter,
// not via javac, so semantic validity is not required.
//
// var declaration with two wildcard imports for services that share the same method.
// The initializer is an opaque factory call — the extractor cannot resolve the type from it.
// type_name is None (var), so the matcher falls back to the import filter.
// Both services are imported via wildcards, so the call is ambiguous —
// both appear in PossibleServices.
import software.amazon.awssdk.services.cloudvault.*;
import software.amazon.awssdk.services.datastore.*;

class VarWildcardImportTwoServices {
    void run() {
        var client = getClient();
        client.describeResource(request);
    }
}
