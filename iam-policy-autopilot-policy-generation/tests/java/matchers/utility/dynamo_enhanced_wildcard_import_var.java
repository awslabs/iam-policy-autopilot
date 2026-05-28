// Wildcard import for the enhanced.dynamodb package with a var-declared local variable.
// type_name is None (var), so the type-name fallback cannot help either.
// The only signal is the wildcard import. This fixture demonstrates the gap.
import software.amazon.awssdk.enhanced.dynamodb.*;

class Test {
    void run() {
        var table = DynamoDbTable.builder().build();
        table.putItem(item);
    }
}
