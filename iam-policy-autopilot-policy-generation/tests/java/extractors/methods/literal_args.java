class Test {
    void run() {
        client.stringArg("my-bucket");
        client.intArg(42);
        client.boolArg(true);
        client.nullArg(null);
        client.identArg(request);
    }
}
