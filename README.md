This is a playground project for getting started with Rust, tokio and async / await.

It implements a TCP listener with a limit on the number of concurrently connected clients. As long as the limit is exceeded, any new connection is dropped.

run `cargo run -- -n 2` in one console, and commands like `while true; do; sleep 0.5; telnet localhost 9000; done` in three other console windows. You will see something like:

![screenshot](https://github.com/ikod/testo/blob/main/img/screen.svg?raw=true)

While number of conncurrent connections lower then 2 (value of `-n` option), every connection will succeed. But if number of connections exceed -n value you will see error messages in console and new connecton will be aborted.

![screenshot](https://github.com/ikod/testo/blob/main/img/screen2.svg?raw=true)
