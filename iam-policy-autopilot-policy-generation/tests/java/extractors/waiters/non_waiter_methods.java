class Test {
    void run() {
        client.putObject(req);
        client.listBuckets(req);
        client.getItem(req);
        client.waitForSomething(req);
    }
}
