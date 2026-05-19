import software.amazon.awssdk.core.SdkBytes;
import software.amazon.awssdk.regions.Region;
import software.amazon.awssdk.services.lambda.LambdaClient;
import software.amazon.awssdk.services.lambda.model.*;
import software.amazon.awssdk.services.sts.StsClient;
import software.amazon.awssdk.services.sts.model.*;

import com.fasterxml.jackson.databind.ObjectMapper;

import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

import java.io.File;
import java.nio.charset.StandardCharsets;

public class Script {
    private static final Logger logger = LoggerFactory.getLogger(Script.class);
    private static final ObjectMapper objectMapper = new ObjectMapper();

    // ── Config loading ─────────────────────────────────────────────────────────

    static class RunConfig {
        public String functionName;
        public String logGroupName;
        public String region;
    }

    private static RunConfig loadConfig() throws Exception {
        // config.json lives one directory above the java/ directory (at the run level).
        File configFile = new File(System.getProperty("user.dir"), "../config.json")
                .getCanonicalFile();

        if (!configFile.exists()) {
            throw new RuntimeException(
                "config.json not found at " + configFile.getAbsolutePath() + ".\n" +
                "Deploy the CDK stack first:\n" +
                "  cd ../cdk && bash deploy.sh"
            );
        }
        return objectMapper.readValue(configFile, RunConfig.class);
    }

    // ── Entry point ────────────────────────────────────────────────────────────

    public static void main(String[] args) {
        try {
            RunConfig cfg = loadConfig();
            Region region = Region.of(cfg.region != null ? cfg.region : "us-east-1");

            logger.info("Starting AWS Deployment Monitoring...");
            logger.info("Using function:  {}", cfg.functionName);
            logger.info("Using log group: {}", cfg.logGroupName);
            logger.info("Using region:    {}", region);

            runDemo(cfg, region);

        } catch (Exception e) {
            logger.error("Application failed: {}", e.getMessage());
            System.exit(1);
        }
    }

    // ── Main logic ────────────────────────────────────────────────────────────

    private static void runDemo(RunConfig cfg, Region region) throws Exception {
        StsClient stsClient       = StsClient.builder().region(region).build();
        LambdaClient lambdaClient = LambdaClient.builder().region(region).build();

        try {
            // 1. STS GetCallerIdentity
            logger.info("Getting AWS account information...");
            GetCallerIdentityResponse identity = stsClient.getCallerIdentity();
            logger.info("Running as: {}", identity.arn());

            // 2. Lambda InvokeFunction
            logger.info("Invoking Lambda function: {}", cfg.functionName);
            InvokeResponse invokeResponse = lambdaClient.invoke(InvokeRequest.builder()
                    .functionName(cfg.functionName)
                    .invocationType(InvocationType.REQUEST_RESPONSE)
                    .build());

            int statusCode = invokeResponse.statusCode();
            String payload = invokeResponse.payload().asString(StandardCharsets.UTF_8);
            logger.info("Lambda invocation status: {}", statusCode);
            logger.info("Lambda response: {}", payload);

            if (statusCode != 200) {
                throw new RuntimeException("Lambda invocation returned unexpected status: " + statusCode);
            }

            logger.info("============================================================");
            logger.info("APPLICATION COMPLETED SUCCESSFULLY!");
            logger.info("============================================================");
            logger.info("Resources used:");
            logger.info("  - Lambda Function: {}", cfg.functionName);
            logger.info("  - Log Group:       {}", cfg.logGroupName);
            logger.info("============================================================");
            logger.info("To destroy infrastructure, run: cd ../cdk && npx cdk destroy");

        } finally {
            stsClient.close();
            lambdaClient.close();
        }
    }
}
