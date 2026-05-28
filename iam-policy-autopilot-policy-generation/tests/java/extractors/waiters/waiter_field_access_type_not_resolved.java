class WaiterFieldAccessTypeNotResolved {
    void run(Object x) {
        x.waiter.waitUntilResourceReady(request);
    }
}
