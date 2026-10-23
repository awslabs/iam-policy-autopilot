/*
 * Generates the operation names consumed by the Rust conformance check.
 *
 * This deliberately uses aws-sdk-java-v2's C2J loaders and
 * IntermediateModelBuilder. That is the part of the SDK codegen pipeline that
 * applies customization.config and determines OperationModel#getMethodName().
 * Source emitters run after this model is complete and cannot change the name.
 */

import java.io.BufferedWriter;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.Map;
import java.util.Optional;
import java.util.TreeMap;
import java.util.stream.Stream;
import software.amazon.awssdk.codegen.C2jModels;
import software.amazon.awssdk.codegen.IntermediateModelBuilder;
import software.amazon.awssdk.codegen.internal.Jackson;
import software.amazon.awssdk.codegen.model.config.customization.CustomizationConfig;
import software.amazon.awssdk.codegen.model.intermediate.IntermediateModel;
import software.amazon.awssdk.codegen.model.intermediate.OperationModel;
import software.amazon.awssdk.codegen.model.service.EndpointRuleSetModel;
import software.amazon.awssdk.codegen.model.service.ServiceModel;
import software.amazon.awssdk.codegen.utils.ModelLoaderUtils;

public final class JavaOperationNamesGenerator {
    private static final String SERVICE_MODEL = "service-2.json";
    private static final String CUSTOMIZATION_CONFIG = "customization.config";
    private static final String ENDPOINT_RULE_SET_MODEL = "endpoint-rule-set.json";

    private JavaOperationNamesGenerator() {
    }

    public static void main(String[] args) throws Exception {
        if (args.length != 2) {
            throw new IllegalArgumentException(
                "Usage: JavaOperationNamesGenerator <services-directory> <output-file>");
        }

        Path servicesDirectory = Paths.get(args[0]);
        Path outputFile = Paths.get(args[1]);
        Map<String, String> mappings = new TreeMap<>();

        try (Stream<Path> files = Files.find(
                 servicesDirectory,
                 10,
                 (path, attributes) -> path.getFileName().toString().equals(SERVICE_MODEL))) {
            files.map(Path::getParent).sorted().forEach(modelRoot -> {
                IntermediateModel model = buildIntermediateModel(modelRoot);
                for (OperationModel operation : model.getOperations().values()) {
                    String operationName = operation.getOperationName();
                    String methodName = operation.getMethodName();
                    String previous = mappings.put(operationName, methodName);
                    if (previous != null && !previous.equals(methodName)) {
                        throw new IllegalStateException(
                            "Operation " + operationName + " maps to both "
                                + previous + " and " + methodName);
                    }
                }
            });
        }

        if (mappings.isEmpty()) {
            throw new IllegalStateException(
                "No service models found under " + servicesDirectory);
        }

        Path parent = outputFile.getParent();
        if (parent != null) {
            Files.createDirectories(parent);
        }
        try (BufferedWriter writer =
                 Files.newBufferedWriter(outputFile, StandardCharsets.UTF_8)) {
            Jackson.writeWithObjectMapper(mappings, writer);
        }
    }

    private static IntermediateModel buildIntermediateModel(Path modelRoot) {
        CustomizationConfig customization =
            loadOptional(CustomizationConfig.class,
                         modelRoot.resolve(CUSTOMIZATION_CONFIG),
                         true)
                .orElseGet(CustomizationConfig::create);

        C2jModels models =
            C2jModels.builder()
                     .customizationConfig(customization)
                     .serviceModel(
                         ModelLoaderUtils.loadModel(
                             ServiceModel.class,
                             modelRoot.resolve(SERVICE_MODEL).toFile()))
                     // S3 adds endpoint parameters in customization.config.
                     // IntermediateModelBuilder applies them to this model and
                     // fails if the endpoint rule set is absent.
                     .endpointRuleSetModel(
                         loadOptional(EndpointRuleSetModel.class,
                                      modelRoot.resolve(ENDPOINT_RULE_SET_MODEL),
                                      false)
                             .orElse(null))
                     .build();

        return new IntermediateModelBuilder(models).build();
    }

    private static <T> Optional<T> loadOptional(
        Class<T> type,
        Path path,
        boolean failOnUnknownProperties
    ) {
        return ModelLoaderUtils.loadOptionalModel(
            type, path.toFile(), failOnUnknownProperties);
    }
}
