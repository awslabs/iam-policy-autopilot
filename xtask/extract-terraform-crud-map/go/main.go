package main

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"reflect"
	"runtime"
	"sort"
	"unique"
	"unsafe"

	"github.com/hashicorp/terraform-provider-aws/internal/conns"
	"github.com/hashicorp/terraform-provider-aws/internal/provider/sdkv2"
	tftags "github.com/hashicorp/terraform-provider-aws/internal/tags"
	inttypes "github.com/hashicorp/terraform-provider-aws/internal/types"
	tfunique "github.com/hashicorp/terraform-provider-aws/internal/unique"
)

// ResourceInfo is the minimal per-resource record consumed by the unified
// model build: the resource type (the plan join key) and its CRUD handler
// symbols (resolved to pkg.func entry points for unified-model lookup).
//
// The four CRUD fields are populated for BOTH resource kinds:
//   - SDKv2 resources: the *_without_timeout handler func pointers.
//   - Plugin Framework resources: the Create/Read/Update/Delete methods on the
//     resource struct.
// The JSON keys are framework-agnostic (`create`, not `create_without_timeout`)
// since they just carry a handler symbol regardless of how the provider declares
// it. Schema, timeouts, and other metadata were dropped as unused.
type ResourceInfo struct {
	ResourceType string    `json:"resource_type"`
	Create       string    `json:"create,omitempty"`
	Read         string    `json:"read,omitempty"`
	Update       string    `json:"update,omitempty"`
	Delete       string    `json:"delete,omitempty"`
	Tags         *TagsInfo `json:"tags,omitempty"`
}

// TagsInfo records the transparent-tagging entry points for a resource that
// carries the `@Tags` annotation. The provider's tagging interceptor calls the
// service package's ListTags (on Read/Create/Update) and UpdateTags (on
// Create/Update) OUTSIDE the CRUD handler bodies, so a call graph rooted at the
// CRUD handlers never reaches the tag SDK calls. Emitting these symbols lets the
// model build root extraction at them too, and the consumer apply the
// CRUD-slot => tag-call rule.
//
// Present only when the resource is tagged AND its service package actually
// implements the ListTags/UpdateTags interface (otherwise the framework no-ops,
// so there is no tag SDK call to attribute).
type TagsInfo struct {
	// ResourceType is the @Tags resourceType (e.g. "Bucket"). For services whose
	// ListTags switches on resourceType, this selects the arm.
	ResourceType string `json:"resource_type,omitempty"`
	// IdentifierAttribute is the @Tags identifierAttribute (e.g. "bucket").
	IdentifierAttribute string `json:"identifier_attribute,omitempty"`
	// ListTagsSymbol is the (*servicePackage).ListTags method symbol (tag read),
	// empty if the service package does not implement a lister.
	ListTagsSymbol string `json:"list_tags_symbol,omitempty"`
	// UpdateTagsSymbol is the (*servicePackage).UpdateTags method symbol (tag
	// write), empty if the service package does not implement an updater.
	UpdateTagsSymbol string `json:"update_tags_symbol,omitempty"`
}

// tagsInfoFor builds the TagsInfo for a resource's `@Tags` handle, or returns
// nil when the resource is untagged or its service package implements no tagging
// methods. Both SDKv2 and Plugin Framework resources carry the same
// `unique.Handle[ServicePackageResourceTags]` and route tagging through the same
// servicePackage.ListTags/UpdateTags (via interceptors.HTags), so one helper
// serves both — the caller passes the resource's Tags handle.
func tagsInfoFor(sp conns.ServicePackage, tags unique.Handle[inttypes.ServicePackageResourceTags]) *TagsInfo {
	if tfunique.IsHandleNil(tags) {
		return nil // not a @Tags resource
	}
	tagsMeta := tags.Value()

	info := &TagsInfo{
		ResourceType:        tagsMeta.ResourceType,
		IdentifierAttribute: tagsMeta.IdentifierAttribute,
	}

	// The interceptor probes the service package for either the plain or the
	// resourceType-aware lister/updater interface; if neither is implemented it
	// no-ops, so we only record a symbol when a method is actually present.
	spType := reflect.TypeOf(sp)
	switch sp.(type) {
	case tftags.ServiceTagLister, tftags.ResourceTypeTagLister:
		info.ListTagsSymbol = methodSymbol(spType, "ListTags")
	}
	switch sp.(type) {
	case tftags.ServiceTagUpdater, tftags.ResourceTypeTagUpdater:
		info.UpdateTagsSymbol = methodSymbol(spType, "UpdateTags")
	}

	// No tagging methods at all => framework no-ops => nothing to attribute.
	if info.ListTagsSymbol == "" && info.UpdateTagsSymbol == "" {
		return nil
	}
	return info
}

// frameworkMethodSymbol returns the symbol for a Plugin Framework resource's
// CRUD method, or "" when that slot is a PROMOTED method (inherited via struct
// embedding) rather than one the resource declares itself.
//
// Why skip promoted methods: a resource satisfies the resource.Resource
// interface by either defining a CRUD method or embedding a framework base that
// provides one (e.g. ResourceWithModel embeds withNoOpUpdate[T], WithNoOpDelete
// provides a no-op Delete). Those promoted methods make no SDK calls, and Go
// attributes them to the EMBEDDING resource type, so the symbol we'd emit
// (`pkg.(*fooResource).Update`) has no body in the resource's own package — the
// per-service-package call-graph builder cannot resolve it and aborts. Since the
// method is a no-op anyway, the correct result is an empty slot.
//
// Detection: a promoted method's compiler-generated wrapper reports its source
// location as "<autogenerated>", whereas a method the resource declares itself
// reports its real source file. This is robust to nesting / generics / new
// framework bases without a maintained allowlist — and it preserves loud
// failure: a method the resource genuinely declares is NOT <autogenerated>, so
// if such a symbol is ever unresolvable the build still aborts rather than
// silently dropping a real handler.
func frameworkMethodSymbol(rt reflect.Type, method string) string {
	m, ok := rt.MethodByName(method)
	if !ok {
		return "" // resource does not implement this CRUD method at all
	}
	fn := runtime.FuncForPC(m.Func.Pointer())
	if file, _ := fn.FileLine(fn.Entry()); file == "<autogenerated>" {
		return "" // promoted from an embedded base (no-op); not the resource's own
	}
	return fn.Name()
}

// methodSymbol returns the fully-qualified symbol of a method on a concrete type
// (e.g. ".../internal/service/s3.(*servicePackage).ListTags" or
// ".../internal/service/appsync.(*apiResource).Create"), using the same
// runtime.FuncForPC mechanism as the SDKv2 CRUD handler symbols. Returns "" when
// the type has no such method.
//
// NOTE: we resolve via the TYPE's Method.Func, not value.MethodByName().Pointer().
// A bound method value's Pointer() returns reflect's internal methodValueCall
// trampoline, not the underlying method, so it cannot be symbolized. The
// type-level Method.Func is the real (unbound) method and symbolizes correctly.
func methodSymbol(t reflect.Type, method string) string {
	m, ok := t.MethodByName(method)
	if !ok {
		return ""
	}
	return runtime.FuncForPC(m.Func.Pointer()).Name()
}

// getServicePackagesViaReflection gets service packages by creating a provider
// and extracting them from the Meta using reflection.
//
// ALTERNATIVE: Instead of using reflection, you can create a simple export file:
//
//	File: internal/provider/sdkv2/service_packages_export.go
//	Content:
//	  package sdkv2
//	  import (
//	      "context"
//	      "github.com/hashicorp/terraform-provider-aws/internal/conns"
//	  )
//	  func ServicePackages(ctx context.Context) []conns.ServicePackage {
//	      return servicePackages(ctx)
//	  }
//	Then replace this function with: return sdkv2.ServicePackages(ctx), nil
//
// The export file approach is:
//   - Cleaner: 3-line wrapper vs complex reflection code
//   - Safer: No unsafe package needed
//   - More maintainable: Explicit intent, won't break on internal changes
//   - Standard Go practice: Common pattern for exposing internal APIs to tools
func getServicePackagesViaReflection(ctx context.Context) ([]conns.ServicePackage, error) {
	fmt.Println("Using reflection to access service packages...")

	// Create a provider instance which will initialize service packages
	provider, err := sdkv2.NewProvider(ctx)
	if err != nil {
		return nil, fmt.Errorf("failed to create provider: %w", err)
	}

	// Get the Meta which contains the service packages
	meta := provider.Meta()
	if meta == nil {
		return nil, fmt.Errorf("provider meta is nil")
	}

	// Cast to AWSClient
	client, ok := meta.(*conns.AWSClient)
	if !ok {
		return nil, fmt.Errorf("meta is not *conns.AWSClient, got %T", meta)
	}

	// Use reflection to access the unexported servicePackages field
	clientValue := reflect.ValueOf(client).Elem()

	// Look for the servicePackages field (it's a map[string]conns.ServicePackage)
	var servicePackagesField reflect.Value
	found := false

	for i := 0; i < clientValue.NumField(); i++ {
		field := clientValue.Field(i)
		fieldType := clientValue.Type().Field(i)

		// Check if this is a map with ServicePackage values
		if field.Kind() == reflect.Map && field.Type().Elem().String() == "conns.ServicePackage" {
			servicePackagesField = field
			found = true
			fmt.Printf("Found service packages field: %s\n", fieldType.Name)
			break
		}
	}

	if !found {
		return nil, fmt.Errorf("could not find service packages map in AWSClient")
	}

	// If the field is unexported, we need to use unsafe to access it
	if !servicePackagesField.CanInterface() {
		// Create a new value that we can interface with using unsafe
		servicePackagesField = reflect.NewAt(
			servicePackagesField.Type(),
			unsafe.Pointer(servicePackagesField.UnsafeAddr()),
		).Elem()
	}

	// Extract the service packages from the map
	packages := make([]conns.ServicePackage, 0, servicePackagesField.Len())
	iter := servicePackagesField.MapRange()
	for iter.Next() {
		pkg := iter.Value().Interface().(conns.ServicePackage)
		packages = append(packages, pkg)
	}

	fmt.Printf("Successfully extracted %d service packages via reflection\n", len(packages))
	return packages, nil
}

func main() {
	if len(os.Args) < 2 {
		fmt.Fprintf(os.Stderr, "Usage: %s <output-file.json>\n", os.Args[0])
		os.Exit(1)
	}

	outputFile := os.Args[1]

	fmt.Println("Extracting resource schemas from Terraform AWS Provider...")
	fmt.Println("Note: Using reflection to access internal service packages")

	ctx := context.Background()

	// Get service packages via reflection
	servicePackages, err := getServicePackagesViaReflection(ctx)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error getting service packages via reflection: %v\n", err)
		os.Exit(1)
	}

	// Collect every resource (SDKv2 + Plugin Framework) into one map keyed by
	// resource type, so the two frameworks are unified in the output. Each entry
	// resolves to the same ResourceInfo shape.
	resourceMap := make(map[string]ResourceInfo)

	for _, sp := range servicePackages {
		// --- SDKv2 resources: CRUD handlers are *_without_timeout func fields ---
		for _, res := range sp.SDKResources(ctx) {
			r := res.Factory()
			resourceMap[res.TypeName] = ResourceInfo{
				ResourceType: res.TypeName,
				Create:       getFunctionName(r.CreateWithoutTimeout),
				Read:         getFunctionName(r.ReadWithoutTimeout),
				Update:       getFunctionName(r.UpdateWithoutTimeout),
				Delete:       getFunctionName(r.DeleteWithoutTimeout),
				Tags:         tagsInfoFor(sp, res.Tags),
			}
		}

		// --- Plugin Framework resources: CRUD are Create/Read/Update/Delete
		// methods on the resource struct. Symbolize them via reflection on the
		// instantiated resource's type (same runtime.FuncForPC mechanism). Tagging
		// uses the SAME servicePackage.ListTags/UpdateTags as SDKv2, so tagsInfoFor
		// is reused unchanged. ---
		for _, res := range sp.FrameworkResources(ctx) {
			instance, err := res.Factory(ctx)
			if err != nil {
				fmt.Fprintf(os.Stderr, "Warning: framework factory for %s failed: %v\n", res.TypeName, err)
				continue
			}
			rt := reflect.TypeOf(instance)
			resourceMap[res.TypeName] = ResourceInfo{
				ResourceType: res.TypeName,
				Create:       frameworkMethodSymbol(rt, "Create"),
				Read:         frameworkMethodSymbol(rt, "Read"),
				Update:       frameworkMethodSymbol(rt, "Update"),
				Delete:       frameworkMethodSymbol(rt, "Delete"),
				Tags:         tagsInfoFor(sp, res.Tags),
			}
		}
	}

	resources := make([]ResourceInfo, 0, len(resourceMap))

	// Get sorted list of resource names for consistent output
	resourceNames := make([]string, 0, len(resourceMap))
	for name := range resourceMap {
		resourceNames = append(resourceNames, name)
	}
	sort.Strings(resourceNames)

	for _, resourceName := range resourceNames {
		resources = append(resources, resourceMap[resourceName])
		fmt.Printf("  Extracted: %s\n", resourceName)
	}

	// Write to JSON file
	file, err := os.Create(outputFile)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error creating output file: %v\n", err)
		os.Exit(1)
	}
	defer file.Close()

	encoder := json.NewEncoder(file)
	encoder.SetIndent("", "  ")
	if err := encoder.Encode(resources); err != nil {
		fmt.Fprintf(os.Stderr, "Error encoding JSON: %v\n", err)
		os.Exit(1)
	}

	fmt.Printf("\nSuccessfully extracted %d resources to %s\n", len(resources), outputFile)
}

func getFunctionName(i interface{}) string {
	if i == nil {
		return ""
	}
	fullName := runtime.FuncForPC(reflect.ValueOf(i).Pointer()).Name()
	return fullName
}
