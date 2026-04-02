// NOTE: This file does not compile — `request` is undeclared.
// It is a syntactic test fixture only; the extractor works on source text via tree-sitter,
// not via javac, so semantic validity is not required.
//
// var declaration with a single wildcard import.
// type_name is None (var), so the matcher falls back to the import filter.
// The wildcard import for s3 is the only candidate, so the call is pinned to s3.
import software.amazon.awssdk.services.s3.*;

class VarWildcardImportSingleService {
    void run() {
        var client = S3Client.builder().build();
        client.getObject(request);
    }
}
