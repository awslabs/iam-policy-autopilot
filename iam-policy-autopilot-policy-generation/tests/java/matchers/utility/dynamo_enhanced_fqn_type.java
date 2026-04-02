// No import — receiver type is fully-qualified in the variable declaration.
class Test {
    void run(software.amazon.awssdk.enhanced.dynamodb.DynamoDbTable<Customer> table) {
        table.putItem(item);
    }
}
