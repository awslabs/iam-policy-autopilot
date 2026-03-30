//! Proc-macro derive for automatic telemetry event generation.
//!
//! # Usage on enums (CLI Commands)
//!
//! ```ignore
//! #[derive(TelemetryEvent)]
//! enum Commands {
//!     #[telemetry(command = "generate-policies")]
//!     GeneratePolicies {
//!         #[telemetry(presence)]
//!         source_files: Vec<PathBuf>,
//!         #[telemetry(value)]
//!         pretty: bool,
//!         #[telemetry(skip)]
//!         debug: bool,
//!     },
//!     #[telemetry(skip)]
//!     Version { verbose: bool },
//! }
//! ```
//!
//! # Usage on structs (MCP tool inputs)
//!
//! ```ignore
//! #[derive(TelemetryEvent)]
//! #[telemetry(command = "mcp-tool-generate-policies")]
//! struct GeneratePoliciesInput {
//!     #[telemetry(presence)]
//!     source_files: Vec<String>,
//!     #[telemetry(value, if_present)]
//!     region: Option<String>,
//! }
//! ```
//!
//! # Container Attributes (on enums/structs/variants)
//!
//! | Attribute | Behavior |
//! |-----------|----------|
//! | `#[telemetry(command = "name")]` | Sets the telemetry command name |
//! | `#[telemetry(skip)]` | Skips telemetry entirely (returns None) |
//! | `#[telemetry(skip_notice)]` | Suppresses CLI telemetry notice for this variant |
//!
//! # Field Attributes
//!
//! | Attribute | Telemetry behavior |
//! |-----------|-------------------|
//! | `#[telemetry(skip)]` | Field is not collected |
//! | `#[telemetry(value)]` | Records the actual value |
//! | `#[telemetry(presence)]` | Records presence as boolean |
//! | `#[telemetry(presence, default = "x")]` | Records `value != "x"` (String fields only) |
//! | `#[telemetry(value, if_present)]` | Records value if `Some`, omits if `None` (Option fields) |
//! | `#[telemetry(list)]` | Records as list if non-empty, omits otherwise (Option<Vec<String>> fields) |
//! | (no attribute) | Field is skipped |

use std::fmt;

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, Data, DeriveInput, Expr, Fields, Lit, Meta};

/// Derive macro for generating `ToTelemetryEvent` implementations.
#[proc_macro_derive(TelemetryEvent, attributes(telemetry))]
pub fn derive_telemetry_event(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    let result = match &input.data {
        Data::Enum(data_enum) => derive_for_enum(&input, data_enum),
        Data::Struct(data_struct) => derive_for_struct(&input, data_struct),
        Data::Union(_) => Err(syn::Error::new_spanned(
            &input,
            "TelemetryEvent cannot be derived for unions",
        )),
    };

    match result {
        Ok(tokens) => tokens,
        Err(err) => err.to_compile_error().into(),
    }
}

// --- Attribute parsing ---

/// Known container-level attribute keywords inside `#[telemetry(...)]`.
const KNOWN_CONTAINER_ATTRS: &[&str] = &["skip", "skip_notice", "command"];

/// Known field-level attribute keywords inside `#[telemetry(...)]`.
const KNOWN_FIELD_ATTRS: &[&str] = &["skip", "value", "presence", "if_present", "list", "default"];

/// Parsed container-level attributes from `#[telemetry(...)]` on enums, structs, or variants.
///
/// These control whether telemetry is emitted for the container, what command name is used,
/// and whether the CLI telemetry notice should be suppressed.
#[derive(Debug)]
struct ContainerAttrs {
    /// The telemetry command name (e.g., `"generate-policies"`).
    /// If `None`, defaults to the lowercased variant/struct name.
    command: Option<String>,
    /// If `true`, telemetry is completely skipped for this variant/struct (returns `None`).
    skip: bool,
    /// If `true`, the CLI telemetry notice is suppressed for this variant/struct.
    /// Used for commands that handle notification differently (e.g., MCP server).
    skip_notice: bool,
}

/// Describes how a single field should be recorded in telemetry.
///
/// Determined by parsing `#[telemetry(...)]` attributes on struct/enum fields.
/// Fields without a `#[telemetry(...)]` attribute default to `Skip`.
#[derive(Debug)]
enum FieldMode {
    /// Field is not collected in telemetry.
    Skip,
    /// Records the actual value (bool as boolean, everything else as string via `.to_string()`).
    Value,
    /// Records whether the field is "present" (non-empty, non-None) as a boolean.
    /// If `default` is `Some(val)`, records `field != val` instead.
    Presence { default: Option<String> },
    /// Records the value if the `Option` field is `Some`, omits the field entirely if `None`.
    ValueIfPresent,
    /// Records `Option<Vec<String>>` as a JSON array if non-empty, omits otherwise.
    List,
}

/// Human-readable description of each `FieldMode`, used to populate
/// [`TelemetryFieldInfo::collection_mode`] for auto-generated documentation tables.
impl fmt::Display for FieldMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FieldMode::Skip => write!(f, "not collected"),
            FieldMode::Value => write!(f, "actual value"),
            FieldMode::Presence { default: None } => write!(f, "presence (boolean)"),
            FieldMode::Presence { default: Some(_) } => write!(f, "whether non-default (boolean)"),
            FieldMode::ValueIfPresent => write!(f, "value if provided, omitted otherwise"),
            FieldMode::List => write!(f, "list of values if non-empty, omitted otherwise"),
        }
    }
}

/// Extract the identifier name from a `syn::Meta` item, regardless of variant.
///
/// Used to retrieve the keyword name from attribute arguments so we can validate
/// it against the known attribute lists (`KNOWN_CONTAINER_ATTRS`, `KNOWN_FIELD_ATTRS`).
///
/// - `Meta::Path(skip)` → `Some("skip")`
/// - `Meta::NameValue(command = "foo")` → `Some("command")`
/// - `Meta::List(something(...))` → `Some("something")`
fn meta_ident_name(meta: &Meta) -> Option<String> {
    match meta {
        Meta::Path(path) => path.get_ident().map(|id| id.to_string()),
        Meta::NameValue(nv) => nv.path.get_ident().map(|id| id.to_string()),
        Meta::List(list) => list.path.get_ident().map(|id| id.to_string()),
    }
}

/// Parse container-level `#[telemetry(...)]` attributes from a slice of `syn::Attribute`.
///
/// Scans all attributes for `#[telemetry(...)]`, extracting:
/// - `skip` → sets `ContainerAttrs::skip = true`
/// - `skip_notice` → sets `ContainerAttrs::skip_notice = true`
/// - `command = "name"` → sets `ContainerAttrs::command = Some("name")`
///
/// Returns a `syn::Error` if an unrecognized keyword is found inside `#[telemetry(...)]`,
/// providing a compile-time error with the span pointing to the offending attribute.
fn parse_container_attrs(attrs: &[syn::Attribute]) -> syn::Result<ContainerAttrs> {
    let mut result = ContainerAttrs {
        command: None,
        skip: false,
        skip_notice: false,
    };

    for attr in attrs {
        if !attr.path().is_ident("telemetry") {
            continue;
        }

        let nested = attr.parse_args_with(
            syn::punctuated::Punctuated::<Meta, syn::Token![,]>::parse_terminated,
        )?;

        for meta in &nested {
            let name = meta_ident_name(meta);
            match meta {
                Meta::Path(path) if path.is_ident("skip") => {
                    result.skip = true;
                }
                Meta::Path(path) if path.is_ident("skip_notice") => {
                    result.skip_notice = true;
                }
                Meta::NameValue(nv) if nv.path.is_ident("command") => {
                    if let Expr::Lit(expr_lit) = &nv.value {
                        if let Lit::Str(lit_str) = &expr_lit.lit {
                            result.command = Some(lit_str.value());
                        }
                    }
                }
                _ => {
                    if let Some(name) = name {
                        if !KNOWN_CONTAINER_ATTRS.contains(&name.as_str()) {
                            return Err(syn::Error::new_spanned(
                                meta,
                                format!(
                                    "unknown telemetry container attribute `{name}`. \
                                     Known attributes: {}",
                                    KNOWN_CONTAINER_ATTRS.join(", ")
                                ),
                            ));
                        }
                    }
                }
            }
        }
    }

    Ok(result)
}

/// Parse field-level `#[telemetry(...)]` attributes to determine how a field is recorded.
///
/// Scans all attributes for `#[telemetry(...)]`, extracting flags like `skip`, `value`,
/// `presence`, `if_present`, `list`, and `default = "..."`. The flags are combined with
/// the following priority: `skip` > `list` > `value + if_present` > `value` > `presence`.
///
/// Fields without any `#[telemetry(...)]` attribute default to `FieldMode::Skip`.
///
/// Returns a `syn::Error` if an unrecognized keyword is found, providing a compile-time
/// error that points to the exact offending token.
fn parse_field_mode(attrs: &[syn::Attribute]) -> syn::Result<FieldMode> {
    for attr in attrs {
        if !attr.path().is_ident("telemetry") {
            continue;
        }

        let nested = attr.parse_args_with(
            syn::punctuated::Punctuated::<Meta, syn::Token![,]>::parse_terminated,
        )?;

        let mut has_skip = false;
        let mut has_value = false;
        let mut has_presence = false;
        let mut has_if_present = false;
        let mut has_list = false;
        let mut default_val: Option<String> = None;

        for meta in &nested {
            let name = meta_ident_name(meta);
            match meta {
                Meta::Path(path) if path.is_ident("skip") => has_skip = true,
                Meta::Path(path) if path.is_ident("value") => has_value = true,
                Meta::Path(path) if path.is_ident("presence") => has_presence = true,
                Meta::Path(path) if path.is_ident("if_present") => has_if_present = true,
                Meta::Path(path) if path.is_ident("list") => has_list = true,
                Meta::NameValue(nv) if nv.path.is_ident("default") => {
                    if let Expr::Lit(expr_lit) = &nv.value {
                        if let Lit::Str(lit_str) = &expr_lit.lit {
                            default_val = Some(lit_str.value());
                        }
                    }
                }
                _ => {
                    if let Some(name) = name {
                        if !KNOWN_FIELD_ATTRS.contains(&name.as_str()) {
                            return Err(syn::Error::new_spanned(
                                meta,
                                format!(
                                    "unknown telemetry field attribute `{name}`. \
                                     Known attributes: {}",
                                    KNOWN_FIELD_ATTRS.join(", ")
                                ),
                            ));
                        }
                    }
                }
            }
        }

        if has_skip {
            return Ok(FieldMode::Skip);
        }
        if has_list {
            return Ok(FieldMode::List);
        }
        if has_value && has_if_present {
            return Ok(FieldMode::ValueIfPresent);
        }
        if has_value {
            return Ok(FieldMode::Value);
        }
        if has_presence {
            return Ok(FieldMode::Presence {
                default: default_val,
            });
        }
    }

    Ok(FieldMode::Skip) // No telemetry attribute = skip
}

// --- Code generation helpers ---

/// Check if a `syn::Type` represents the `bool` type.
///
/// Used by [`generate_field_code`] to determine whether to emit `with_bool()` (for booleans)
/// or `with_str()` (for everything else) when recording a `FieldMode::Value` field.
fn is_bool_type(ty: &syn::Type) -> bool {
    if let syn::Type::Path(type_path) = ty {
        type_path.path.is_ident("bool")
    } else {
        false
    }
}

/// Generate field recording code for a single field.
///
/// Three accessor token streams handle the different access patterns between enums and structs:
///
/// - `accessor`: base access (enum: `#name` via `ref` binding; struct: `self.#field`)
/// - `deref_accessor`: dereferenced access for value copies (enum: `*#name`; struct: `self.#field`)
/// - `ref_accessor`: borrowed access for trait methods expecting `&T` (enum: `#name`; struct: `&self.#field`)
fn generate_field_code(
    accessor: &proc_macro2::TokenStream,
    deref_accessor: &proc_macro2::TokenStream,
    ref_accessor: &proc_macro2::TokenStream,
    name_str: &str,
    field_type: &syn::Type,
    mode: &FieldMode,
) -> Option<proc_macro2::TokenStream> {
    match mode {
        FieldMode::Skip => None,
        FieldMode::Value => {
            if is_bool_type(field_type) {
                Some(quote! { event = event.with_bool(#name_str, #deref_accessor); })
            } else {
                Some(quote! { event = event.with_str(#name_str, #accessor.to_string()); })
            }
        }
        FieldMode::Presence { default: None } => Some(quote! {
            event = event.with_telemetry_presence(#name_str, #ref_accessor);
        }),
        FieldMode::Presence {
            default: Some(default_val),
        } => Some(quote! {
            event = event.with_bool(#name_str, #deref_accessor != #default_val);
        }),
        FieldMode::ValueIfPresent => Some(quote! {
            if let Some(ref val) = #accessor {
                event = event.with_str(#name_str, val.to_string());
            }
        }),
        FieldMode::List => Some(quote! {
            if let Some(ref items) = #accessor {
                if !items.is_empty() {
                    event = event.with_list(#name_str, items);
                }
            }
        }),
    }
}

/// Generate a wildcard match pattern for an enum variant based on its field shape.
///
/// Produces the correct destructuring syntax for each variant kind:
/// - `Fields::Named` → `EnumName::Variant { .. }`
/// - `Fields::Unnamed` → `EnumName::Variant(..)`
/// - `Fields::Unit` → `EnumName::Variant`
///
/// Used for skip arms, notice arms, and non-skip variants without capturable named fields.
fn wildcard_pattern(
    enum_name: &syn::Ident,
    variant_name: &syn::Ident,
    fields: &Fields,
) -> proc_macro2::TokenStream {
    match fields {
        Fields::Named(_) => quote! { #enum_name::#variant_name { .. } },
        Fields::Unnamed(_) => quote! { #enum_name::#variant_name(..) },
        Fields::Unit => quote! { #enum_name::#variant_name },
    }
}

// --- Code generation: enums ---

/// Generate the `ToTelemetryEvent` implementation for an enum.
///
/// For each variant:
/// - Skipped variants (`#[telemetry(skip)]`) return `None` from `to_telemetry_event()`
/// - Non-skip variants with named fields destructure and record each field
///   according to its `FieldMode`
/// - Non-skip variants with unnamed/unit fields emit only the command event (no field data)
///
/// Also generates:
/// - `telemetry_fields()` — metadata about all collected fields for documentation
/// - `should_skip_notice()` — per-variant check for notice suppression
fn derive_for_enum(
    input: &DeriveInput,
    data_enum: &syn::DataEnum,
) -> syn::Result<TokenStream> {
    let enum_name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let mut variant_arms = Vec::new();
    let mut field_info_entries = Vec::new();
    let mut skip_notice_arms = Vec::new();

    for variant in &data_enum.variants {
        let variant_name = &variant.ident;
        let container_attrs = parse_container_attrs(&variant.attrs)?;

        // --- should_skip_notice arm ---
        let should_skip = container_attrs.skip || container_attrs.skip_notice;
        let wildcard = wildcard_pattern(enum_name, variant_name, &variant.fields);
        skip_notice_arms.push(quote! { #wildcard => #should_skip, });

        // --- to_telemetry_event arm ---
        if container_attrs.skip {
            variant_arms.push(quote! { #wildcard => None, });
            continue;
        }

        let command_name = container_attrs
            .command
            .unwrap_or_else(|| variant_name.to_string().to_lowercase());

        // --- telemetry_fields entries ---
        if let Fields::Named(fields_named) = &variant.fields {
            for field in &fields_named.named {
                let name = field.ident.as_ref().expect("named field").to_string();
                let mode = parse_field_mode(&field.attrs)?;
                let mode_str = mode.to_string();
                field_info_entries.push(quote! {
                    iam_policy_autopilot_common::telemetry::TelemetryFieldInfo {
                        command: #command_name.to_string(),
                        field_name: #name.to_string(),
                        collection_mode: #mode_str.to_string(),
                    }
                });
            }
        }

        // --- to_telemetry_event arm for non-skip variant ---
        if let Fields::Named(fields_named) = &variant.fields {
            let mut captured_names = Vec::new();
            let mut field_code = Vec::new();

            for field in &fields_named.named {
                let name = field.ident.as_ref().expect("named field");
                let mode = parse_field_mode(&field.attrs)?;

                let accessor = quote! { #name };
                let deref_accessor = quote! { *#name };
                // Enum fields are bound with `ref`, so they're already references
                let ref_accessor = quote! { #name };
                if let Some(code) =
                    generate_field_code(&accessor, &deref_accessor, &ref_accessor, &name.to_string(), &field.ty, &mode)
                {
                    captured_names.push(quote! { ref #name });
                    field_code.push(code);
                }
            }

            if field_code.is_empty() {
                variant_arms.push(quote! {
                    #enum_name::#variant_name { .. } => {
                        Some(iam_policy_autopilot_common::telemetry::TelemetryEvent::new(#command_name))
                    }
                });
            } else {
                variant_arms.push(quote! {
                    #enum_name::#variant_name { #(#captured_names,)* .. } => {
                        let mut event = iam_policy_autopilot_common::telemetry::TelemetryEvent::new(#command_name);
                        #(#field_code)*
                        Some(event)
                    }
                });
            }
        } else {
            // Unnamed or unit variants — no fields to capture, just emit the command event
            let pattern = wildcard_pattern(enum_name, variant_name, &variant.fields);
            variant_arms.push(quote! {
                #pattern => {
                    Some(iam_policy_autopilot_common::telemetry::TelemetryEvent::new(#command_name))
                }
            });
        }
    }

    let expanded = quote! {
        impl #impl_generics iam_policy_autopilot_common::telemetry::ToTelemetryEvent for #enum_name #ty_generics #where_clause {
            fn to_telemetry_event(&self) -> Option<iam_policy_autopilot_common::telemetry::TelemetryEvent> {
                if !iam_policy_autopilot_common::telemetry::is_telemetry_enabled() {
                    return None;
                }
                match self {
                    #(#variant_arms)*
                }
            }

            fn telemetry_fields() -> Vec<iam_policy_autopilot_common::telemetry::TelemetryFieldInfo> {
                vec![
                    #(#field_info_entries,)*
                ]
            }

            fn should_skip_notice(&self) -> bool {
                match self {
                    #(#skip_notice_arms)*
                }
            }
        }
    };

    Ok(expanded.into())
}

// --- Code generation: structs ---

/// Generate the `ToTelemetryEvent` implementation for a struct.
///
/// Reads the struct-level `#[telemetry(command = "...")]` attribute to determine the command
/// name (falls back to the lowercased struct name). Iterates over all named fields, recording
/// each according to its `FieldMode`.
///
/// Also generates:
/// - `telemetry_fields()` — metadata about all fields for documentation
/// - `should_skip_notice()` — returns `true` if `skip` or `skip_notice` is set
fn derive_for_struct(
    input: &DeriveInput,
    data_struct: &syn::DataStruct,
) -> syn::Result<TokenStream> {
    let struct_name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    let container_attrs = parse_container_attrs(&input.attrs)?;

    let command_name = container_attrs
        .command
        .unwrap_or_else(|| struct_name.to_string().to_lowercase());

    let skip_notice = container_attrs.skip || container_attrs.skip_notice;

    let mut field_code = Vec::new();
    let mut struct_field_info = Vec::new();

    if let Fields::Named(fields_named) = &data_struct.fields {
        for field in &fields_named.named {
            let field_name = field.ident.as_ref().expect("named field");
            let name_str = field_name.to_string();
            let mode = parse_field_mode(&field.attrs)?;
            let mode_str = mode.to_string();

            // Collect field info for telemetry_fields()
            struct_field_info.push(quote! {
                iam_policy_autopilot_common::telemetry::TelemetryFieldInfo {
                    command: #command_name.to_string(),
                    field_name: #name_str.to_string(),
                    collection_mode: #mode_str.to_string(),
                }
            });

            // Collect field recording code for to_telemetry_event()
            let accessor = quote! { self.#field_name };
            let deref_accessor = quote! { self.#field_name };
            // Struct fields need explicit borrowing for trait method calls
            let ref_accessor = quote! { &self.#field_name };
            if let Some(code) =
                generate_field_code(&accessor, &deref_accessor, &ref_accessor, &name_str, &field.ty, &mode)
            {
                field_code.push(code);
            }
        }
    }

    let expanded = quote! {
        impl #impl_generics iam_policy_autopilot_common::telemetry::ToTelemetryEvent for #struct_name #ty_generics #where_clause {
            fn to_telemetry_event(&self) -> Option<iam_policy_autopilot_common::telemetry::TelemetryEvent> {
                if !iam_policy_autopilot_common::telemetry::is_telemetry_enabled() {
                    return None;
                }
                let mut event = iam_policy_autopilot_common::telemetry::TelemetryEvent::new(#command_name);
                #(#field_code)*
                Some(event)
            }

            fn telemetry_fields() -> Vec<iam_policy_autopilot_common::telemetry::TelemetryFieldInfo> {
                vec![
                    #(#struct_field_info,)*
                ]
            }

            fn should_skip_notice(&self) -> bool {
                #skip_notice
            }
        }
    };

    Ok(expanded.into())
}

// =============================================================================
// Unit tests for internal helper functions
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_quote;

    // --- FieldMode Display tests ---

    #[test]
    fn field_mode_display_skip() {
        assert_eq!(FieldMode::Skip.to_string(), "not collected");
    }

    #[test]
    fn field_mode_display_value() {
        assert_eq!(FieldMode::Value.to_string(), "actual value");
    }

    #[test]
    fn field_mode_display_presence_no_default() {
        assert_eq!(
            FieldMode::Presence { default: None }.to_string(),
            "presence (boolean)"
        );
    }

    #[test]
    fn field_mode_display_presence_with_default() {
        assert_eq!(
            FieldMode::Presence {
                default: Some("x".to_string())
            }
            .to_string(),
            "whether non-default (boolean)"
        );
    }

    #[test]
    fn field_mode_display_value_if_present() {
        assert_eq!(
            FieldMode::ValueIfPresent.to_string(),
            "value if provided, omitted otherwise"
        );
    }

    #[test]
    fn field_mode_display_list() {
        assert_eq!(
            FieldMode::List.to_string(),
            "list of values if non-empty, omitted otherwise"
        );
    }

    // --- is_bool_type tests ---

    #[test]
    fn is_bool_type_returns_true_for_bool() {
        let ty: syn::Type = parse_quote!(bool);
        assert!(is_bool_type(&ty));
    }

    #[test]
    fn is_bool_type_returns_false_for_string() {
        let ty: syn::Type = parse_quote!(String);
        assert!(!is_bool_type(&ty));
    }

    #[test]
    fn is_bool_type_returns_false_for_option() {
        let ty: syn::Type = parse_quote!(Option<bool>);
        assert!(!is_bool_type(&ty));
    }

    #[test]
    fn is_bool_type_returns_false_for_vec() {
        let ty: syn::Type = parse_quote!(Vec<String>);
        assert!(!is_bool_type(&ty));
    }

    // --- meta_ident_name tests ---

    #[test]
    fn meta_ident_name_for_path() {
        let meta: Meta = parse_quote!(skip);
        assert_eq!(meta_ident_name(&meta), Some("skip".to_string()));
    }

    #[test]
    fn meta_ident_name_for_name_value() {
        let meta: Meta = parse_quote!(command = "foo");
        assert_eq!(meta_ident_name(&meta), Some("command".to_string()));
    }

    #[test]
    fn meta_ident_name_for_list() {
        let meta: Meta = parse_quote!(something(a, b));
        assert_eq!(meta_ident_name(&meta), Some("something".to_string()));
    }

    // --- parse_container_attrs tests ---

    fn make_telemetry_attr(tokens: proc_macro2::TokenStream) -> syn::Attribute {
        parse_quote!(#[telemetry(#tokens)])
    }

    #[test]
    fn parse_container_attrs_empty() {
        let attrs: Vec<syn::Attribute> = vec![];
        let result = parse_container_attrs(&attrs).unwrap();
        assert!(!result.skip);
        assert!(!result.skip_notice);
        assert!(result.command.is_none());
    }

    #[test]
    fn parse_container_attrs_skip() {
        let attrs = vec![make_telemetry_attr(quote!(skip))];
        let result = parse_container_attrs(&attrs).unwrap();
        assert!(result.skip);
        assert!(!result.skip_notice);
    }

    #[test]
    fn parse_container_attrs_skip_notice() {
        let attrs = vec![make_telemetry_attr(quote!(skip_notice))];
        let result = parse_container_attrs(&attrs).unwrap();
        assert!(!result.skip);
        assert!(result.skip_notice);
    }

    #[test]
    fn parse_container_attrs_command() {
        let attrs = vec![make_telemetry_attr(quote!(command = "my-cmd"))];
        let result = parse_container_attrs(&attrs).unwrap();
        assert_eq!(result.command, Some("my-cmd".to_string()));
    }

    #[test]
    fn parse_container_attrs_combined() {
        let attrs = vec![make_telemetry_attr(quote!(skip_notice, command = "mcp-server"))];
        let result = parse_container_attrs(&attrs).unwrap();
        assert!(result.skip_notice);
        assert_eq!(result.command, Some("mcp-server".to_string()));
    }

    #[test]
    fn parse_container_attrs_ignores_non_telemetry() {
        let attrs: Vec<syn::Attribute> = vec![parse_quote!(#[serde(rename_all = "PascalCase")])];
        let result = parse_container_attrs(&attrs).unwrap();
        assert!(!result.skip);
        assert!(result.command.is_none());
    }

    #[test]
    fn parse_container_attrs_unknown_keyword_errors() {
        let attrs = vec![make_telemetry_attr(quote!(bogus))];
        let result = parse_container_attrs(&attrs);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("unknown telemetry container attribute `bogus`"),
            "error should mention the unknown attribute: {msg}"
        );
    }

    // --- parse_field_mode tests ---

    #[test]
    fn parse_field_mode_no_attrs_returns_skip() {
        let attrs: Vec<syn::Attribute> = vec![];
        let mode = parse_field_mode(&attrs).unwrap();
        assert!(matches!(mode, FieldMode::Skip));
    }

    #[test]
    fn parse_field_mode_skip() {
        let attrs = vec![make_telemetry_attr(quote!(skip))];
        let mode = parse_field_mode(&attrs).unwrap();
        assert!(matches!(mode, FieldMode::Skip));
    }

    #[test]
    fn parse_field_mode_value() {
        let attrs = vec![make_telemetry_attr(quote!(value))];
        let mode = parse_field_mode(&attrs).unwrap();
        assert!(matches!(mode, FieldMode::Value));
    }

    #[test]
    fn parse_field_mode_presence() {
        let attrs = vec![make_telemetry_attr(quote!(presence))];
        let mode = parse_field_mode(&attrs).unwrap();
        assert!(matches!(mode, FieldMode::Presence { default: None }));
    }

    #[test]
    fn parse_field_mode_presence_with_default() {
        let attrs = vec![make_telemetry_attr(quote!(presence, default = "us-east-1"))];
        let mode = parse_field_mode(&attrs).unwrap();
        match mode {
            FieldMode::Presence { default: Some(val) } => {
                assert_eq!(val, "us-east-1");
            }
            _ => panic!("expected Presence with default, got: {mode}"),
        }
    }

    #[test]
    fn parse_field_mode_value_if_present() {
        let attrs = vec![make_telemetry_attr(quote!(value, if_present))];
        let mode = parse_field_mode(&attrs).unwrap();
        assert!(matches!(mode, FieldMode::ValueIfPresent));
    }

    #[test]
    fn parse_field_mode_list() {
        let attrs = vec![make_telemetry_attr(quote!(list))];
        let mode = parse_field_mode(&attrs).unwrap();
        assert!(matches!(mode, FieldMode::List));
    }

    #[test]
    fn parse_field_mode_skip_takes_priority() {
        // Even if both skip and value are present, skip wins
        let attrs = vec![make_telemetry_attr(quote!(skip, value))];
        let mode = parse_field_mode(&attrs).unwrap();
        assert!(matches!(mode, FieldMode::Skip));
    }

    #[test]
    fn parse_field_mode_unknown_keyword_errors() {
        let attrs = vec![make_telemetry_attr(quote!(foobar))];
        let result = parse_field_mode(&attrs);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("unknown telemetry field attribute `foobar`"),
            "error should mention the unknown attribute: {msg}"
        );
    }

    #[test]
    fn parse_field_mode_ignores_non_telemetry_attrs() {
        let attrs: Vec<syn::Attribute> = vec![parse_quote!(#[serde(rename = "Foo")])];
        let mode = parse_field_mode(&attrs).unwrap();
        assert!(matches!(mode, FieldMode::Skip));
    }

    // --- wildcard_pattern tests ---

    #[test]
    fn wildcard_pattern_named_fields() {
        let enum_name: syn::Ident = parse_quote!(Commands);
        let variant_name: syn::Ident = parse_quote!(Generate);
        let fields: Fields = Fields::Named(parse_quote!({ x: i32, y: String }));

        let pattern = wildcard_pattern(&enum_name, &variant_name, &fields);
        let expected = quote!(Commands::Generate { .. });
        assert_eq!(pattern.to_string(), expected.to_string());
    }

    #[test]
    fn wildcard_pattern_unnamed_fields() {
        let enum_name: syn::Ident = parse_quote!(Commands);
        let variant_name: syn::Ident = parse_quote!(Internal);
        let fields: Fields = Fields::Unnamed(parse_quote!((String)));

        let pattern = wildcard_pattern(&enum_name, &variant_name, &fields);
        let expected = quote!(Commands::Internal(..));
        assert_eq!(pattern.to_string(), expected.to_string());
    }

    #[test]
    fn wildcard_pattern_unit() {
        let enum_name: syn::Ident = parse_quote!(Commands);
        let variant_name: syn::Ident = parse_quote!(Help);

        let pattern = wildcard_pattern(&enum_name, &variant_name, &Fields::Unit);
        let expected = quote!(Commands::Help);
        assert_eq!(pattern.to_string(), expected.to_string());
    }

    // --- generate_field_code tests ---

    #[test]
    fn generate_field_code_skip_returns_none() {
        let accessor = quote!(field);
        let deref = quote!(*field);
        let ref_acc = quote!(field);
        let ty: syn::Type = parse_quote!(String);
        assert!(generate_field_code(&accessor, &deref, &ref_acc, "field", &ty, &FieldMode::Skip).is_none());
    }

    #[test]
    fn generate_field_code_value_bool_emits_with_bool() {
        let accessor = quote!(self.pretty);
        let deref = quote!(self.pretty);
        let ref_acc = quote!(&self.pretty);
        let ty: syn::Type = parse_quote!(bool);
        let code = generate_field_code(&accessor, &deref, &ref_acc, "pretty", &ty, &FieldMode::Value)
            .expect("should produce code");
        let code_str = code.to_string();
        assert!(
            code_str.contains("with_bool"),
            "bool value should use with_bool: {code_str}"
        );
    }

    #[test]
    fn generate_field_code_value_string_emits_with_str() {
        let accessor = quote!(self.language);
        let deref = quote!(self.language);
        let ref_acc = quote!(&self.language);
        let ty: syn::Type = parse_quote!(String);
        let code = generate_field_code(&accessor, &deref, &ref_acc, "language", &ty, &FieldMode::Value)
            .expect("should produce code");
        let code_str = code.to_string();
        assert!(
            code_str.contains("with_str"),
            "String value should use with_str: {code_str}"
        );
    }

    #[test]
    fn generate_field_code_presence_emits_with_telemetry_presence() {
        let accessor = quote!(self.files);
        let deref = quote!(self.files);
        let ref_acc = quote!(&self.files);
        let ty: syn::Type = parse_quote!(Vec<String>);
        let code = generate_field_code(
            &accessor,
            &deref,
            &ref_acc,
            "files",
            &ty,
            &FieldMode::Presence { default: None },
        )
        .expect("should produce code");
        let code_str = code.to_string();
        assert!(
            code_str.contains("with_telemetry_presence"),
            "presence should use with_telemetry_presence: {code_str}"
        );
    }

    #[test]
    fn generate_field_code_presence_with_default_emits_comparison() {
        let accessor = quote!(self.region);
        let deref = quote!(self.region);
        let ref_acc = quote!(&self.region);
        let ty: syn::Type = parse_quote!(String);
        let code = generate_field_code(
            &accessor,
            &deref,
            &ref_acc,
            "region",
            &ty,
            &FieldMode::Presence {
                default: Some("us-east-1".to_string()),
            },
        )
        .expect("should produce code");
        let code_str = code.to_string();
        assert!(
            code_str.contains("with_bool") && code_str.contains("us-east-1"),
            "presence with default should compare against default: {code_str}"
        );
    }

    #[test]
    fn generate_field_code_value_if_present_emits_option_match() {
        let accessor = quote!(self.region);
        let deref = quote!(self.region);
        let ref_acc = quote!(&self.region);
        let ty: syn::Type = parse_quote!(Option<String>);
        let code = generate_field_code(&accessor, &deref, &ref_acc, "region", &ty, &FieldMode::ValueIfPresent)
            .expect("should produce code");
        let code_str = code.to_string();
        assert!(
            code_str.contains("Some"),
            "value_if_present should match on Some: {code_str}"
        );
    }

    #[test]
    fn generate_field_code_list_emits_list_check() {
        let accessor = quote!(self.hints);
        let deref = quote!(self.hints);
        let ref_acc = quote!(&self.hints);
        let ty: syn::Type = parse_quote!(Option<Vec<String>>);
        let code = generate_field_code(&accessor, &deref, &ref_acc, "hints", &ty, &FieldMode::List)
            .expect("should produce code");
        let code_str = code.to_string();
        assert!(
            code_str.contains("with_list") && code_str.contains("is_empty"),
            "list should check non-empty and use with_list: {code_str}"
        );
    }
}
