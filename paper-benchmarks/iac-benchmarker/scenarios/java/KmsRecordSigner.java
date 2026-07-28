package com.example.auditlog;

import software.amazon.awssdk.core.SdkBytes;
import software.amazon.awssdk.services.kms.KmsClient;
import software.amazon.awssdk.services.kms.model.*;

import java.nio.charset.StandardCharsets;
import java.util.Base64;
import java.util.logging.Logger;

/**
 * Signs audit log records using AWS KMS.
 *
 * <p>Each record is signed with the configured KMS asymmetric key using
 * RSASSA_PKCS1_V1_5_SHA_256.  The resulting signature is Base64-encoded
 * and attached to the record envelope before archiving.
 */
public class KmsRecordSigner {

    private static final Logger LOG = Logger.getLogger(KmsRecordSigner.class.getName());
    private static final SigningAlgorithmSpec ALGORITHM = SigningAlgorithmSpec.RSASSA_PKCS1_V1_5_SHA_256;

    private final KmsClient kms;
    private final String keyId;

    public KmsRecordSigner(KmsClient kms, AppConfig cfg) {
        this.kms   = kms;
        this.keyId = cfg.kmsKeyId;
    }

    /**
     * Sign the UTF-8 bytes of {@code payload} and return a Base64-encoded
     * signature string.
     */
    public String sign(String payload) {
        byte[] messageBytes = payload.getBytes(StandardCharsets.UTF_8);

        var resp = kms.sign(SignRequest.builder()
            .keyId(keyId)
            .message(SdkBytes.fromByteArray(messageBytes))
            .messageType(MessageType.RAW)
            .signingAlgorithm(ALGORITHM)
            .build());

        String signature = Base64.getEncoder().encodeToString(resp.signature().asByteArray());
        LOG.fine("Signed record (" + messageBytes.length + " bytes) → " + signature.length() + " char signature");
        return signature;
    }

    /**
     * Verify a previously produced signature.
     *
     * @return {@code true} if the signature is valid.
     */
    public boolean verify(String payload, String base64Signature) {
        byte[] messageBytes   = payload.getBytes(StandardCharsets.UTF_8);
        byte[] signatureBytes = Base64.getDecoder().decode(base64Signature);

        try {
            var resp = kms.verify(VerifyRequest.builder()
                .keyId(keyId)
                .message(SdkBytes.fromByteArray(messageBytes))
                .messageType(MessageType.RAW)
                .signature(SdkBytes.fromByteArray(signatureBytes))
                .signingAlgorithm(ALGORITHM)
                .build());
            return Boolean.TRUE.equals(resp.signatureValid());
        } catch (KmsInvalidSignatureException e) {
            LOG.warning("Signature verification failed: " + e.getMessage());
            return false;
        }
    }

    /**
     * Generate a data key for envelope encryption (AES-256).
     * Returns the plaintext key bytes — caller must zeroize after use.
     */
    public byte[] generateDataKey() {
        var resp = kms.generateDataKey(GenerateDataKeyRequest.builder()
            .keyId(keyId)
            .keySpec(DataKeySpec.AES_256)
            .build());
        return resp.plaintext().asByteArray();
    }

    /**
     * Describe the KMS key to confirm it is enabled and usable.
     */
    public void validateKey() {
        var resp = kms.describeKey(DescribeKeyRequest.builder().keyId(keyId).build());
        var meta = resp.keyMetadata();
        if (!meta.enabled()) {
            throw new IllegalStateException("KMS key " + keyId + " is not enabled (state: " + meta.keyState() + ")");
        }
        LOG.info("KMS key validated: " + meta.keyId() + " (" + meta.keyUsage() + ")");
    }
}
