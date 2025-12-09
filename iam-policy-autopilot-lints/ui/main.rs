// Test cases for node_kind_literal lint

struct Node;

impl Node {
    fn kind(&self) -> &str {
        "test"
    }
}

fn test_kind_comparisons() {
    let node = Node;
    
    // This should trigger a warning - .kind() compared with string literal
    if node.kind() == "composite_literal" {
        println!("found composite literal");
    }
    
    // This should trigger a warning - reversed comparison
    if "unary_expression" == node.kind() {
        println!("found unary expression");
    }
    
    // This should trigger a warning - inequality comparison
    if node.kind() != "literal_value" {
        println!("not a literal value");
    }
    
    // This should trigger a warning - any string literal with .kind()
    if node.kind() == "some_new_node_type" {
        println!("found new node type");
    }
}

fn test_allowed_comparisons() {
    let node = Node;
    
    // These should NOT trigger warnings (not comparing with .kind())
    let name = "my_function";
    let message = "Hello, world!";
    
    // This is fine - not comparing with .kind()
    if name == "test" {
        println!("{}", message);
    }
    
    // This is fine - just assigning a string
    let node_kind_value = "composite_literal";
    println!("{}", node_kind_value);
    
    // This is fine - comparing .kind() with a constant (not a literal)
    const EXPECTED_KIND: &str = "expected";
    if node.kind() == EXPECTED_KIND {
        println!("matched expected kind");
    }
}

fn main() {
    test_kind_comparisons();
    test_allowed_comparisons();
}
