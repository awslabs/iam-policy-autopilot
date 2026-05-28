import software.amazon.awssdk.services.resourcegroups.ResourceGroupsClient;
import software.amazon.awssdk.services.resourcegroups.model.*;
import software.amazon.awssdk.services.xray.XRayClient;
import software.amazon.awssdk.services.xray.model.*;

// Reproduces the bug where createGroup is ambiguous between resource-groups and xray.
//
// The ResourceGroupsClient is stored as a class field without an inline initializer;
// it is assigned in the constructor.  Because the field declaration has no initializer,
// find_receiver_binding returns None (no ReceiverBinding is produced), so the matcher
// falls to the Tier-2 import-filter path.  Both "resource-groups" and "xray" are
// imported, and both services have a createGroup operation, so the import filter cannot
// disambiguate and returns both services.
class CreateGroupBug {
    private final ResourceGroupsClient resourceGroupsClient;
    private final XRayClient xrayClient;

    CreateGroupBug() {
        this.resourceGroupsClient = ResourceGroupsClient.builder().build();
        this.xrayClient = XRayClient.builder().build();
    }

    void run() {
        resourceGroupsClient.createGroup(CreateGroupRequest.builder().name("my-group").build());
    }
}
