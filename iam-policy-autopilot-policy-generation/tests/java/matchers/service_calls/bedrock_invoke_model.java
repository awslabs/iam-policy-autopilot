import software.amazon.awssdk.services.bedrockruntime.BedrockRuntimeClient;
import software.amazon.awssdk.services.bedrockruntime.model.InvokeModelRequest;
import software.amazon.awssdk.services.bedrockruntime.model.InvokeModelResponse;

class Test {
    void run(BedrockRuntimeClient bedrockClient) {
        InvokeModelResponse response = bedrockClient.invokeModel(InvokeModelRequest.builder().build());
    }
}
