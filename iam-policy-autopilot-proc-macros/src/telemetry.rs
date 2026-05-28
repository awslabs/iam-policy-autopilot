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
//! | `#[telemetry(count)]` | Records the length of a Vec as an integer |
//! | `#[telemetry(value, if_present)]` | Records value if `Some`, omits if `None` (Option fields) |
//! | `#[telemetry(list)]` | Records as list if non-empty, omits otherwise (Option<Vec<String>> fields) |
//! | (no attribute) | Field is skipped |

use std::fmt;

use quote::quote;
use syn::{Data, DeriveInput, Expr, Fields, Lit, Meta};

/// Implementation of the `TelemetryEvent` derive macro.
///
/// Called from `lib.rs` (the proc-macro entry point). Accepts a parsed `DeriveInput`
/// and returns either the generated `impl` token stream or a compile error.
pub(crate) fn derive_telemetry_event_impl(input: DeriveInput) -> proc_macro2::TokenStream {
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
        Err(err) => err.to_compile_error(),
    }
}

// --- Attribute parsing ---

/// Known container-level attribute keywords inside `#[telemetry(...)]`.
const KNOWN_CONTAINER_ATTRS: &[&str] = &["skip", "skip_notice", "command"];

/// Known field-level attribute keywords inside `#[telemetry(...)]`.
const KNOWN_FIELD_ATTRS: &[&str] = &[
    "skip",
    "value",
    "presence",
    "count",
    "if_present",
    "list",
    "default",
];

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
    /// Records the length of a `Vec` as an integer.
    Count,
    /// Records the value if the `Option` field is `Some`, omits the field entirely if `None`.
    ValueIfPresent,
    /// Records `Option<Vec<String>>` as a JSON array if non-empty, omits otherwise.
    List,
}

/// Human-readable description of each `FieldMode` without type information.
/// Used for debug output and test assertions.
impl fmt::Display for FieldMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Skip => write!(f, "not collected"),
            Self::Value => write!(f, "actual value"),
            Self::Presence { default: None } => write!(f, "presence (boolean)"),
            Self::Presence { default: Some(_) } => write!(f, "whether non-default (boolean)"),
            Self::Count => write!(f, "count of items"),
            Self::ValueIfPresent => write!(f, "value if provided, omitted otherwise"),
            Self::List => write!(f, "list of values if non-empty, omitted otherwise"),
        }
    }
}

/// Produce a human-readable collection mode description that includes the value type
/// for `FieldMode::Value`. Used to populate [`TelemetryFieldInfo::collection_mode`]
/// in the auto-generated documentation tables.
///
/// Examples:
/// - `Value` + `bool` → `"actual value (boolean)"`
/// - `Value` + `String` → `"actual value (string)"`
/// - `Value` + `u32` → `"actual value (u32)"`
/// - `Presence` → `"presence (boolean)"` (type not relevant)
fn collection_mode_description(mode: &FieldMode, field_type: &syn::Type) -> String {
    match mode {
        FieldMode::Value => {
            let type_label = friendly_type_name(field_type);
            format!("actual value ({type_label})")
        }
        // All other modes have fixed descriptions independent of the field type
        other => other.to_string(),
    }
}

/// Convert a `syn::Type` to a short human-readable label for documentation.
///
/// Recognizes common types like `bool`, `String`, `usize`, and falls back to
/// the raw type token string for anything else.
fn friendly_type_name(ty: &syn::Type) -> String {
    if let syn::Type::Path(type_path) = ty {
        if let Some(ident) = type_path.path.get_ident() {
            let name = ident.to_string();
            return match name.as_str() {
                "bool" => "boolean".to_string(),
                "String" => "string".to_string(),
                "usize" | "u8" | "u16" | "u32" | "u64" | "u128" | "isize" | "i8" | "i16"
                | "i32" | "i64" | "i128" => name,
                "f32" | "f64" => name,
                _ => name,
            };
        }
    }
    // For complex types (generics, references, etc.), use the token representation
    quote::quote!(#ty).to_string()
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
        Meta::Path(path) => path.get_ident().map(ToString::to_string),
        Meta::NameValue(nv) => nv.path.get_ident().map(ToString::to_string),
        Meta::List(list) => list.path.get_ident().map(ToString::to_string),
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

        // Phase 1: Collect all recognized flags from the comma-separated attribute list.
        // e.g., `#[telemetry(value, if_present)]` sets `has_value = true` and `has_if_present = true`.
        let mut has_skip = false;
        let mut has_value = false;
        let mut has_presence = false;
        let mut has_count = false;
        let mut has_if_present = false;
        let mut has_list = false;
        let mut default_val: Option<String> = None;

        for meta in &nested {
            // Extract the keyword name for error reporting on unrecognized attributes
            let name = meta_ident_name(meta);
            match meta {
                // Bare keywords: `skip`, `value`, `presence`, `if_present`, `list`
                Meta::Path(path) if path.is_ident("skip") => has_skip = true,
                Meta::Path(path) if path.is_ident("value") => has_value = true,
                Meta::Path(path) if path.is_ident("presence") => has_presence = true,
                Meta::Path(path) if path.is_ident("count") => has_count = true,
                Meta::Path(path) if path.is_ident("if_present") => has_if_present = true,
                Meta::Path(path) if path.is_ident("list") => has_list = true,
                // Key-value: `default = "some_string"` — only valid with `presence`
                Meta::NameValue(nv) if nv.path.is_ident("default") => {
                    if let Expr::Lit(expr_lit) = &nv.value {
                        if let Lit::Str(lit_str) = &expr_lit.lit {
                            default_val = Some(lit_str.value());
                        }
                    }
                }
                // Catch-all: reject unknown keywords with a helpful compile error
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

        // Phase 2: Resolve the collected flags into a single FieldMode.
        // Priority order ensures deterministic behavior when multiple flags are present:
        //   skip > list > (value + if_present) > value > presence
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
        if has_count {
            return Ok(FieldMode::Count);
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
        FieldMode::Count => Some(quote! {
            event = event.with_number(#name_str, #accessor.len());
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
) -> syn::Result<proc_macro2::TokenStream> {
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
                let field_name_str = field.ident.as_ref().expect("named field").to_string();
                let mode = parse_field_mode(&field.attrs)?;
                let mode_description = collection_mode_description(&mode, &field.ty);
                field_info_entries.push(quote! {
                    iam_policy_autopilot_common::telemetry::TelemetryFieldInfo {
                        command: #command_name.to_string(),
                        field_name: #field_name_str.to_string(),
                        collection_mode: #mode_description.to_string(),
                    }
                });
            }
        }

        // --- to_telemetry_event arm for non-skip variant ---
        if let Fields::Named(fields_named) = &variant.fields {
            let mut ref_bindings = Vec::new();
            let mut field_recording_code = Vec::new();

            for field in &fields_named.named {
                let field_ident = field.ident.as_ref().expect("named field");
                let mode = parse_field_mode(&field.attrs)?;

                let accessor = quote! { #field_ident };
                let deref_accessor = quote! { *#field_ident };
                // Enum fields are bound with `ref`, so they're already references
                let ref_accessor = quote! { #field_ident };
                if let Some(code) = generate_field_code(
                    &accessor,
                    &deref_accessor,
                    &ref_accessor,
                    &field_ident.to_string(),
                    &field.ty,
                    &mode,
                ) {
                    ref_bindings.push(quote! { ref #field_ident });
                    field_recording_code.push(code);
                }
            }

            if field_recording_code.is_empty() {
                variant_arms.push(quote! {
                    #enum_name::#variant_name { .. } => {
                        Some(iam_policy_autopilot_common::telemetry::TelemetryEvent::new(#command_name))
                    }
                });
            } else {
                variant_arms.push(quote! {
                    #enum_name::#variant_name { #(#ref_bindings,)* .. } => {
                        let mut event = iam_policy_autopilot_common::telemetry::TelemetryEvent::new(#command_name);
                        #(#field_recording_code)*
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

    Ok(expanded)
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
) -> syn::Result<proc_macro2::TokenStream> {
    let struct_name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    let container_attrs = parse_container_attrs(&input.attrs)?;

    let command_name = container_attrs
        .command
        .unwrap_or_else(|| struct_name.to_string().to_lowercase());

    let skip_notice = container_attrs.skip || container_attrs.skip_notice;

    let mut field_recording_code = Vec::new();
    let mut field_info_entries = Vec::new();

    if let Fields::Named(fields_named) = &data_struct.fields {
        for field in &fields_named.named {
            let field_ident = field.ident.as_ref().expect("named field");
            let field_name_str = field_ident.to_string();
            let mode = parse_field_mode(&field.attrs)?;
            let mode_description = collection_mode_description(&mode, &field.ty);

            // Collect field info for telemetry_fields()
            field_info_entries.push(quote! {
                iam_policy_autopilot_common::telemetry::TelemetryFieldInfo {
                    command: #command_name.to_string(),
                    field_name: #field_name_str.to_string(),
                    collection_mode: #mode_description.to_string(),
                }
            });

            // Collect field recording code for to_telemetry_event()
            let accessor = quote! { self.#field_ident };
            let deref_accessor = quote! { self.#field_ident };
            // Struct fields need explicit borrowing for trait method calls
            let ref_accessor = quote! { &self.#field_ident };
            if let Some(code) = generate_field_code(
                &accessor,
                &deref_accessor,
                &ref_accessor,
                &field_name_str,
                &field.ty,
                &mode,
            ) {
                field_recording_code.push(code);
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
                #(#field_recording_code)*
                Some(event)
            }

            fn telemetry_fields() -> Vec<iam_policy_autopilot_common::telemetry::TelemetryFieldInfo> {
                vec![
                    #(#field_info_entries,)*
                ]
            }

            fn should_skip_notice(&self) -> bool {
                #skip_notice
            }
        }
    };

    Ok(expanded)
}

// =============================================================================
// Unit tests for internal helper functions
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use syn::parse_quote;

    // --- Shared test helper ---

    /// Create a `#[telemetry(...)]` attribute with the given token content.
    fn make_telemetry_attr(tokens: proc_macro2::TokenStream) -> syn::Attribute {
        parse_quote!(#[telemetry(#tokens)])
    }

    // =========================================================================
    // FieldMode::Display — parameterized over all variants
    // =========================================================================

    #[rstest]
    #[case::skip(FieldMode::Skip, "not collected")]
    #[case::value(FieldMode::Value, "actual value")]
    #[case::presence(FieldMode::Presence { default: None }, "presence (boolean)")]
    #[case::presence_with_default(
        FieldMode::Presence { default: Some("x".to_string()) },
        "whether non-default (boolean)"
    )]
    #[case::count(FieldMode::Count, "count of items")]
    #[case::value_if_present(FieldMode::ValueIfPresent, "value if provided, omitted otherwise")]
    #[case::list(FieldMode::List, "list of values if non-empty, omitted otherwise")]
    fn field_mode_display(#[case] mode: FieldMode, #[case] expected: &str) {
        assert_eq!(mode.to_string(), expected);
    }

    // =========================================================================
    // is_bool_type — parameterized over common types
    // =========================================================================

    #[rstest]
    #[case::bool_type("bool", true)]
    #[case::string_type("String", false)]
    #[case::option_bool("Option<bool>", false)]
    #[case::vec_string("Vec<String>", false)]
    #[case::i32_type("i32", false)]
    fn is_bool_type_detection(#[case] type_str: &str, #[case] expected: bool) {
        let ty: syn::Type = syn::parse_str(type_str).expect("valid type");
        assert_eq!(is_bool_type(&ty), expected, "is_bool_type({type_str})");
    }

    // =========================================================================
    // meta_ident_name — parameterized over Meta variants
    // =========================================================================

    #[rstest]
    #[case::path("skip", "skip")]
    #[case::name_value("command = \"foo\"", "command")]
    #[case::list("something(a, b)", "something")]
    fn meta_ident_name_extraction(#[case] input: &str, #[case] expected: &str) {
        let meta: Meta = syn::parse_str(input).expect("valid meta");
        assert_eq!(
            meta_ident_name(&meta),
            Some(expected.to_string()),
            "meta_ident_name for `{input}`"
        );
    }

    // =========================================================================
    // parse_container_attrs — valid cases
    // =========================================================================

    #[rstest]
    #[case::empty(vec![], false, false, None)]
    #[case::skip(
        vec![make_telemetry_attr(quote!(skip))],
        true, false, None
    )]
    #[case::skip_notice(
        vec![make_telemetry_attr(quote!(skip_notice))],
        false, true, None
    )]
    #[case::command(
        vec![make_telemetry_attr(quote!(command = "my-cmd"))],
        false, false, Some("my-cmd".to_string())
    )]
    #[case::combined(
        vec![make_telemetry_attr(quote!(skip_notice, command = "mcp-server"))],
        false, true, Some("mcp-server".to_string())
    )]
    fn parse_container_attrs_valid(
        #[case] attrs: Vec<syn::Attribute>,
        #[case] expect_skip: bool,
        #[case] expect_skip_notice: bool,
        #[case] expect_command: Option<String>,
    ) {
        let result = parse_container_attrs(&attrs).expect("should parse successfully");
        assert_eq!(result.skip, expect_skip, "skip mismatch");
        assert_eq!(
            result.skip_notice, expect_skip_notice,
            "skip_notice mismatch"
        );
        assert_eq!(result.command, expect_command, "command mismatch");
    }

    #[test]
    fn parse_container_attrs_ignores_non_telemetry_attributes() {
        let attrs: Vec<syn::Attribute> = vec![parse_quote!(#[serde(rename_all = "PascalCase")])];
        let result = parse_container_attrs(&attrs).expect("should parse successfully");
        assert!(!result.skip);
        assert!(result.command.is_none());
    }

    #[test]
    fn parse_container_attrs_rejects_unknown_keyword() {
        let attrs = vec![make_telemetry_attr(quote!(bogus))];
        let err = parse_container_attrs(&attrs).unwrap_err().to_string();
        assert!(
            err.contains("unknown telemetry container attribute `bogus`"),
            "error should mention the unknown attribute: {err}"
        );
    }

    // =========================================================================
    // parse_field_mode — valid cases
    // =========================================================================

    /// Helper to express expected `FieldMode` variants as strings for `rstest` cases,
    /// since `FieldMode` doesn't implement `PartialEq`.
    fn assert_field_mode_matches(mode: &FieldMode, expected_pattern: &str) {
        match (mode, expected_pattern) {
            (FieldMode::Skip, "Skip") => {}
            (FieldMode::Value, "Value") => {}
            (FieldMode::Count, "Count") => {}
            (FieldMode::Presence { default: None }, "Presence(None)") => {}
            (FieldMode::Presence { default: Some(val) }, expected)
                if expected.starts_with("Presence(Some(") =>
            {
                let expected_val = expected
                    .strip_prefix("Presence(Some(")
                    .and_then(|s| s.strip_suffix("))"))
                    .expect("malformed expected pattern");
                assert_eq!(val, expected_val, "default value mismatch");
            }
            (FieldMode::ValueIfPresent, "ValueIfPresent") => {}
            (FieldMode::List, "List") => {}
            _ => panic!("FieldMode {mode:?} did not match expected pattern `{expected_pattern}`"),
        }
    }

    #[rstest]
    #[case::no_attrs(vec![], "Skip")]
    #[case::skip(vec![make_telemetry_attr(quote!(skip))], "Skip")]
    #[case::value(vec![make_telemetry_attr(quote!(value))], "Value")]
    #[case::presence(vec![make_telemetry_attr(quote!(presence))], "Presence(None)")]
    #[case::count(vec![make_telemetry_attr(quote!(count))], "Count")]
    #[case::presence_with_default(
        vec![make_telemetry_attr(quote!(presence, default = "us-east-1"))],
        "Presence(Some(us-east-1))"
    )]
    #[case::value_if_present(
        vec![make_telemetry_attr(quote!(value, if_present))],
        "ValueIfPresent"
    )]
    #[case::list(vec![make_telemetry_attr(quote!(list))], "List")]
    #[case::skip_takes_priority(
        vec![make_telemetry_attr(quote!(skip, value))],
        "Skip"
    )]
    fn parse_field_mode_valid(#[case] attrs: Vec<syn::Attribute>, #[case] expected: &str) {
        let mode = parse_field_mode(&attrs).expect("should parse successfully");
        assert_field_mode_matches(&mode, expected);
    }

    #[test]
    fn parse_field_mode_ignores_non_telemetry_attributes() {
        let attrs: Vec<syn::Attribute> = vec![parse_quote!(#[serde(rename = "Foo")])];
        let mode = parse_field_mode(&attrs).expect("should parse");
        assert!(matches!(mode, FieldMode::Skip));
    }

    #[test]
    fn parse_field_mode_rejects_unknown_keyword() {
        let attrs = vec![make_telemetry_attr(quote!(foobar))];
        let err = parse_field_mode(&attrs).unwrap_err().to_string();
        assert!(
            err.contains("unknown telemetry field attribute `foobar`"),
            "error should mention the unknown attribute: {err}"
        );
    }

    // =========================================================================
    // wildcard_pattern — parameterized over field shapes
    // =========================================================================

    #[rstest]
    #[case::named(
        Fields::Named(parse_quote!({ x: i32, y: String })),
        "Commands :: Generate { .. }"
    )]
    #[case::unnamed(
        Fields::Unnamed(parse_quote!((String))),
        "Commands :: Generate (..)"
    )]
    #[case::unit(Fields::Unit, "Commands :: Generate")]
    fn wildcard_pattern_for_field_shapes(#[case] fields: Fields, #[case] expected: &str) {
        let enum_name: syn::Ident = parse_quote!(Commands);
        let variant_name: syn::Ident = parse_quote!(Generate);
        let pattern = wildcard_pattern(&enum_name, &variant_name, &fields);
        assert_eq!(pattern.to_string(), expected);
    }

    // =========================================================================
    // generate_field_code — parameterized over FieldMode variants
    // =========================================================================

    /// Test case definition for `generate_field_code` parameterized tests.
    struct FieldCodeTestCase {
        mode: FieldMode,
        field_type: &'static str,
        expected_output: Option<&'static str>, // None = should return None; Some(needle) = output should contain needle
    }

    #[rstest]
    #[case::skip(FieldCodeTestCase {
        mode: FieldMode::Skip,
        field_type: "String",
        expected_output: None,
    })]
    #[case::value_bool(FieldCodeTestCase {
        mode: FieldMode::Value,
        field_type: "bool",
        expected_output: Some("with_bool"),
    })]
    #[case::value_string(FieldCodeTestCase {
        mode: FieldMode::Value,
        field_type: "String",
        expected_output: Some("with_str"),
    })]
    #[case::presence(FieldCodeTestCase {
        mode: FieldMode::Presence { default: None },
        field_type: "Vec<String>",
        expected_output: Some("with_telemetry_presence"),
    })]
    #[case::count(FieldCodeTestCase {
        mode: FieldMode::Count,
        field_type: "Vec<String>",
        expected_output: Some("with_number"),
    })]
    #[case::presence_with_default(FieldCodeTestCase {
        mode: FieldMode::Presence { default: Some("us-east-1".to_string()) },
        field_type: "String",
        expected_output: Some("us-east-1"),
    })]
    #[case::value_if_present(FieldCodeTestCase {
        mode: FieldMode::ValueIfPresent,
        field_type: "Option<String>",
        expected_output: Some("Some"),
    })]
    #[case::list(FieldCodeTestCase {
        mode: FieldMode::List,
        field_type: "Option<Vec<String>>",
        expected_output: Some("with_list"),
    })]
    fn generate_field_code_for_mode(#[case] test_case: FieldCodeTestCase) {
        let accessor = quote!(self.field);
        let deref_accessor = quote!(self.field);
        let ref_accessor = quote!(&self.field);
        let field_type: syn::Type = syn::parse_str(test_case.field_type).expect("valid type");

        let result = generate_field_code(
            &accessor,
            &deref_accessor,
            &ref_accessor,
            "field",
            &field_type,
            &test_case.mode,
        );

        match test_case.expected_output {
            None => {
                assert!(
                    result.is_none(),
                    "FieldMode::{:?} should produce no code",
                    test_case.mode
                );
            }
            Some(needle) => {
                let code_str = result
                    .unwrap_or_else(|| {
                        panic!("FieldMode::{:?} should produce code", test_case.mode)
                    })
                    .to_string();
                assert!(
                    code_str.contains(needle),
                    "FieldMode::{:?} output should contain `{needle}`, got: {code_str}",
                    test_case.mode
                );
            }
        }
    }

    // =========================================================================
    // friendly_type_name — parameterized over common types
    // =========================================================================

    #[rstest]
    #[case::bool_type("bool", "boolean")]
    #[case::string_type("String", "string")]
    #[case::u32_type("u32", "u32")]
    #[case::i64_type("i64", "i64")]
    #[case::f64_type("f64", "f64")]
    #[case::usize_type("usize", "usize")]
    #[case::custom_struct("MyStruct", "MyStruct")]
    fn friendly_type_name_mapping(#[case] type_str: &str, #[case] expected: &str) {
        let ty: syn::Type = syn::parse_str(type_str).expect("valid type");
        assert_eq!(
            friendly_type_name(&ty),
            expected,
            "friendly_type_name({type_str})"
        );
    }

    // =========================================================================
    // collection_mode_description — type-aware mode descriptions
    // =========================================================================

    #[rstest]
    #[case::value_bool(FieldMode::Value, "bool", "actual value (boolean)")]
    #[case::value_string(FieldMode::Value, "String", "actual value (string)")]
    #[case::value_u32(FieldMode::Value, "u32", "actual value (u32)")]
    #[case::skip(FieldMode::Skip, "String", "not collected")]
    #[case::count(FieldMode::Count, "Vec<String>", "count of items")]
    #[case::presence(FieldMode::Presence { default: None }, "Vec<String>", "presence (boolean)")]
    #[case::presence_with_default(
        FieldMode::Presence { default: Some("x".to_string()) },
        "String",
        "whether non-default (boolean)"
    )]
    #[case::value_if_present(
        FieldMode::ValueIfPresent,
        "Option<String>",
        "value if provided, omitted otherwise"
    )]
    #[case::list(
        FieldMode::List,
        "Option<Vec<String>>",
        "list of values if non-empty, omitted otherwise"
    )]
    fn collection_mode_description_with_type(
        #[case] mode: FieldMode,
        #[case] type_str: &str,
        #[case] expected: &str,
    ) {
        let ty: syn::Type = syn::parse_str(type_str).expect("valid type");
        assert_eq!(
            collection_mode_description(&mode, &ty),
            expected,
            "collection_mode_description({mode:?}, {type_str})"
        );
    }
}
