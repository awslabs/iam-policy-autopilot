import software.amazon.awssdk.services.cloudcluster.CloudClusterClient;
import software.amazon.awssdk.services.cloudcluster.waiters.CloudClusterWaiter;
import software.amazon.awssdk.services.datacluster.DataClusterClient;
import software.amazon.awssdk.services.datacluster.waiters.DataClusterWaiter;

/**
 * Regression test for the case where two services share the same waiter name
 * (ClusterActive) but poll different underlying operations:
 *   cloudcluster → DescribeCluster
 *   datacluster  → GetCluster
 *
 * Before the fix, match_waiters() called refs.first().operation_name for the
 * method_name, so whichever service was loaded second would get the wrong
 * operation name (e.g. cloudcluster would emit "getCluster" instead of
 * "describeCluster").
 */
class TwoServiceWaitersDifferentOperations {
    void run(CloudClusterClient cloudClusterClient, DataClusterClient dataClusterClient) {
        CloudClusterWaiter cloudClusterWaiter = cloudClusterClient.waiter();
        DataClusterWaiter dataClusterWaiter = dataClusterClient.waiter();

        cloudClusterWaiter.waitUntilClusterActive(r -> r.build());
        dataClusterWaiter.waitUntilClusterActive(r -> r.build());
    }
}
