// NOTE: This file does not compile — `request` is undeclared.
// It is a syntactic test fixture only; the extractor works on source text via tree-sitter,
// not via javac, so semantic validity is not required.
//
// A non-AWS wildcard import whose package happens to contain a type named "DynamoDbClient"
// with a method named "listTablesPaginator" — both coincidentally matching AWS SDK names.
// The java_service_name fallback must NOT fire because there is no AWS wildcard import
// in the file; the only wildcard is from a non-AWS package.
// Expected: no SDK calls emitted.
import com.example.mylib.*;

class NonAwsWildcardNoMatch {
    void run(DynamoDbClient client) {
        client.listTablesPaginator(request);
    }
}
