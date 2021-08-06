This is a test project for getting started with Rust, tokio and async / await.

It implements a TCP listener with a limit on the number of concurrently connected clients. As long as the limit is exceeded, any new connection is dropped.
