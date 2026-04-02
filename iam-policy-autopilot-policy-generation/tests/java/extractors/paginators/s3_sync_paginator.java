import software.amazon.awssdk.services.s3.S3Client;
import software.amazon.awssdk.services.s3.model.ListObjectsV2Request;
import software.amazon.awssdk.services.s3.paginators.ListObjectsV2Iterable;

class S3SyncPaginator {
    void run(S3Client s3, String bucketName) {
        ListObjectsV2Request listReq = ListObjectsV2Request.builder()
            .bucket(bucketName)
            .maxKeys(1)
            .build();

        ListObjectsV2Iterable listRes = s3.listObjectsV2Paginator(listReq);

        listRes.stream()
            .flatMap(r -> r.contents().stream())
            .forEach(content -> System.out
                .println(" Key: " + content.key() + " size = " + content.size()));
    }
}
