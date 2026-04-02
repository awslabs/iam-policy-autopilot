// Port of the AWS SDK for Java v2 service-name derivation algorithm.
//
// Used at runtime by `ServiceMetadata::java_service_name()` (see `sdk_model.rs`).
//
// Drift detection: `CODEGEN_NAMING_UTILS_EXPECTED_SHA256` in `build.rs` guards against
// upstream changes to `CodegenNamingUtils.java`. The snapshot in
// `tests/java/service_name_test_cases.txt` is verified against the live submodule at build time
// by `verify_service_name_test_cases()` in `build.rs`. To regenerate the snapshot:
//   cargo xtask regenerate-service-names

/// Port of `CodegenNamingUtils.splitOnWordBoundaries()` from aws-sdk-java-v2.
///
/// Drift detection: `CODEGEN_NAMING_UTILS_EXPECTED_SHA256` in `build.rs`.
fn split_on_word_boundaries(to_split: &str) -> Vec<String> {
    use regex::Regex;
    let mut result = to_split.to_string();
    result = Regex::new(r"[^A-Za-z0-9]+")
        .expect("regex is valid")
        .replace_all(&result, " ")
        .into_owned();
    result = Regex::new(r"([^a-z]{2,})v([0-9]+)")
        .expect("regex is valid")
        .replace_all(&result, "$1 v$2 ")
        .into_owned();
    result = Regex::new(r"([^A-Z]{2,})V([0-9]+)")
        .expect("regex is valid")
        .replace_all(&result, "$1 V$2 ")
        .into_owned();
    // Java uses split("(?<=[a-z])(?=[A-Z]([a-zA-Z]|[0-9]))") — lookbehind + lookahead.
    // The `regex` crate does not support lookaround, so we implement this manually:
    // insert a space between a lowercase letter and an uppercase letter that is followed
    // by an alphanumeric character.
    result = split_camel_case(&result);
    result = Regex::new(r"([A-Z]+)([A-Z][a-z])")
        .expect("regex is valid")
        .replace_all(&result, "$1 $2")
        .into_owned();
    result = Regex::new(r"([0-9])([a-zA-Z])")
        .expect("regex is valid")
        .replace_all(&result, "$1 $2")
        .into_owned();
    result = Regex::new(r" +")
        .expect("regex is valid")
        .replace_all(&result, " ")
        .into_owned();
    result = result.trim().to_string();
    result.split(' ').map(str::to_string).collect()
}

/// Implements the Java regex `split("(?<=[a-z])(?=[A-Z]([a-zA-Z]|[0-9]))")` without
/// lookaround. Inserts a space between a lowercase letter and an uppercase letter that is
/// followed by an alphanumeric character.
#[allow(dead_code)]
fn split_camel_case(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len() + 8);
    for (i, &c) in chars.iter().enumerate() {
        out.push(c);
        // Insert space after a lowercase letter if the next char is uppercase
        // and the char after that (if any) is alphanumeric.
        if c.is_ascii_lowercase() {
            if let Some(&next) = chars.get(i + 1) {
                if next.is_ascii_uppercase() {
                    let after_next_is_alnum = chars
                        .get(i + 2)
                        .map(char::is_ascii_alphanumeric)
                        .unwrap_or(false);
                    if after_next_is_alnum {
                        out.push(' ');
                    }
                }
            }
        }
    }
    out
}

/// Port of `CodegenNamingUtils.pascalCase(String)`.
///
/// # Why not `convert_case::Case::Pascal`?
///
/// Two incompatibilities make `convert_case` (v0.8) unsuitable here:
///
/// 1. **Token lowercasing**: the Java pipeline lowercases the *entire* token before capitalizing
///    its first character (`"EC2"` → `"ec2"` → `"Ec2"`). `Case::Pascal` preserves the case of
///    non-first characters, so it would produce `"EC2"`, `"IAM"`, `"ACM"` instead of the
///    required `"Ec2"`, `"Iam"`, `"Acm"`.
///
/// 2. **Version-suffix boundaries**: the Java regexes `([^a-z]{2,})v([0-9]+)` and
///    `([^A-Z]{2,})V([0-9]+)` require a variable-length lookbehind (`{2,}`).
///    `Boundary::Custom` operates on a fixed-width byte window and cannot carry state across
///    positions, so these rules cannot be expressed with the crate's API.
fn pascal_case(word: &str) -> String {
    split_on_word_boundaries(word)
        .iter()
        .map(|w| {
            let lower = w.to_lowercase();
            let mut chars = lower.chars();
            match chars.next() {
                None => String::new(),
                Some(c) => c.to_uppercase().to_string() + chars.as_str(),
            }
        })
        .collect()
}

/// Port of `DefaultNamingStrategy.removeRedundantPrefixesAndSuffixes()`.
/// `Utils.removeLeading` / `removeTrailing` are case-insensitive in the Java SDK.
fn remove_redundant_prefixes_and_suffixes(name: &str) -> String {
    let name = remove_leading_ci(name, "amazon");
    let name = remove_leading_ci(&name, "aws");
    remove_trailing_ci(&name, "service")
}

fn remove_leading_ci(s: &str, prefix: &str) -> String {
    if s.len() >= prefix.len() && s[..prefix.len()].eq_ignore_ascii_case(prefix) {
        s[prefix.len()..].to_string()
    } else {
        s.to_string()
    }
}

fn remove_trailing_ci(s: &str, suffix: &str) -> String {
    if s.len() >= suffix.len() && s[s.len() - suffix.len()..].eq_ignore_ascii_case(suffix) {
        s[..s.len() - suffix.len()].to_string()
    } else {
        s.to_string()
    }
}

/// Full pipeline: `serviceId` → Java SDK service name (client class prefix).
/// Equivalent to `DefaultNamingStrategy.getServiceName()`.
///
/// Examples:
/// - `"EC2"` → `"Ec2"` (→ `Ec2Client`)
/// - `"DynamoDB"` → `"DynamoDb"` (→ `DynamoDbClient`)
/// - `"CloudHSM V2"` → `"CloudHsmV2"` (→ `CloudHsmV2Client`)
/// - `"IAM"` → `"Iam"` (→ `IamClient`)
pub(crate) fn java_service_name(service_id: &str) -> String {
    remove_redundant_prefixes_and_suffixes(&pascal_case(service_id))
}

#[cfg(test)]
mod tests {
    use super::java_service_name;

    /// Test cases sourced from:
    ///   aws-sdk-java-v2/codegen/src/test/java/software/amazon/awssdk/codegen/naming/
    ///   DefaultNamingStrategyTest.java — validateServiceIdentifiersForEnvVarsAndProfileProperty()
    ///   (fourth argument = getServiceName())
    ///
    /// The snapshot file is verified against the live submodule at build time by
    /// `verify_service_name_test_cases()` in `build.rs`. To regenerate:
    ///   cargo xtask regenerate-service-names
    #[test]
    fn test_service_id_to_java_service_name() {
        let snapshot = include_str!("../../../../tests/java/service_name_test_cases.txt");
        for line in snapshot.lines().filter(|l| !l.starts_with('#')) {
            let (service_id, expected) = line
                .split_once('=')
                .unwrap_or_else(|| panic!("malformed line in snapshot: {line:?}"));
            assert_eq!(
                java_service_name(service_id),
                expected,
                "java_service_name({service_id:?}) should be {expected:?}"
            );
        }
    }
}
