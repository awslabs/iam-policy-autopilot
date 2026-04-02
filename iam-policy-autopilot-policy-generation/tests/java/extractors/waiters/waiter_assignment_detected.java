class Test {
    void run() {
        WaiterResponse<HeadBucketResponse> resp = waiter.waitUntilBucketExists(request);
    }
}
