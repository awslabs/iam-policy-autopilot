// Test cases for node_kind_literal and convert_case_pascal lints

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

// Test that non-Node types with kind() methods don't trigger the lint
struct OtherType;

impl OtherType {
    fn kind(&self) -> &str {
        "other"
    }
}

fn test_non_node_kind() {
    let other = OtherType;

    // This should NOT trigger a warning - not a tree-sitter Node type
    if other.kind() == "some_string" {
        println!("other type kind");
    }
}

// Test cases for convert_case_pascal lint

// Mock convert_case types and traits for testing
mod convert_case {
    pub enum Case {
        Pascal,
        Snake,
        Camel,
    }
}

trait Casing {
    fn to_case(&self, case: convert_case::Case) -> String;
}

impl Casing for str {
    fn to_case(&self, _case: convert_case::Case) -> String {
        self.to_string()
    }
}

fn test_convert_case_pascal() {
    let text = "hello_world";
    
    // This should trigger a warning - using Case::Pascal
    let _result1 = text.to_case(convert_case::Case::Pascal);
    
    // This should NOT trigger a warning - using different case
    let _result2 = text.to_case(convert_case::Case::Snake);
    let _result3 = text.to_case(convert_case::Case::Camel);
}

fn test_other_to_case_calls() {
    let text = "test_string";
    
    // This should NOT trigger a warning - not using Case::Pascal
    let _result = text.to_case(convert_case::Case::Snake);
}

fn main() {
    test_kind_comparisons();
    test_allowed_comparisons();
    test_non_node_kind();
    test_convert_case_pascal();
    test_other_to_case_calls();
}
