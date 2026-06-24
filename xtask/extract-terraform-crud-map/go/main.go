package main

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"reflect"
	"runtime"
	"sort"
	"unsafe"

	"github.com/hashicorp/terraform-plugin-sdk/v2/helper/schema"
	"github.com/hashicorp/terraform-provider-aws/internal/conns"
	"github.com/hashicorp/terraform-provider-aws/internal/provider/sdkv2"
)

// ResourceInfo is the minimal per-resource record consumed by the unified
// model build: the resource type (the plan join key) and its CRUD handler
// symbols (resolved to pkg.func entry points for unified-model lookup).
//
// Only the *_without_timeout handlers are populated by SDKv2 resources; the
// legacy *_context / bare CRUD variants, schema, timeouts, and other metadata
// were dropped as unused (they made up ~66% of the output). Add fields back
// here and in main() if a future consumer needs them.
type ResourceInfo struct {
	ResourceType         string `json:"resource_type"`
	CreateWithoutTimeout string `json:"create_without_timeout,omitempty"`
	ReadWithoutTimeout   string `json:"read_without_timeout,omitempty"`
	UpdateWithoutTimeout string `json:"update_without_timeout,omitempty"`
	DeleteWithoutTimeout string `json:"delete_without_timeout,omitempty"`
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

	// Collect all resources from service packages
	resourceMap := make(map[string]*schema.Resource)

	for _, sp := range servicePackages {
		for _, res := range sp.SDKResources(ctx) {
			// Call the factory function to get the unwrapped resource
			resourceMap[res.TypeName] = res.Factory()
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
		resource := resourceMap[resourceName]

		// Only the *_without_timeout handlers are populated by SDKv2 resources.
		info := ResourceInfo{
			ResourceType:         resourceName,
			CreateWithoutTimeout: getFunctionName(resource.CreateWithoutTimeout),
			ReadWithoutTimeout:   getFunctionName(resource.ReadWithoutTimeout),
			UpdateWithoutTimeout: getFunctionName(resource.UpdateWithoutTimeout),
			DeleteWithoutTimeout: getFunctionName(resource.DeleteWithoutTimeout),
		}

		resources = append(resources, info)
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
