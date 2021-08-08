use color_eyre::Report;
use futures::{
    task::{Context, Poll},
    Future,
};
use std::env;
use std::{
    error::Error, fmt, io, net::SocketAddr, pin::Pin, result::Result, sync::Arc, time::Duration,
};
use testo::parse_args;
use tokio::{
    io::AsyncWriteExt,
    net::{TcpListener, TcpStream},
    select,
    sync::{OwnedSemaphorePermit, Semaphore},
    task::JoinHandle,
    time::sleep,
};
use tokio_util::sync::PollSemaphore;
use tracing::{debug, instrument};

#[derive(Debug, PartialEq)]
enum State {
    AcceptConnections,
    WaitForTaskCompletion,
}

struct MyFut<'a> {
    state: State,
    listener: &'a TcpListener,
    accept_poll: Pin<Box<dyn Future<Output = io::Result<(TcpStream, SocketAddr)>> + 'a>>,
    semaphore_poll: PollSemaphore,
    stop_after: usize,
}

impl<'a> MyFut<'a> {
    fn new(listener: &'a TcpListener, pool_size: usize, stop_after: usize) -> Self {
        let accept_poll = Box::pin(listener.accept());
        let semaphore = Arc::new(Semaphore::new(pool_size));
        let semaphore_poll = PollSemaphore::new(semaphore);
        MyFut {
            state: State::AcceptConnections,
            listener,
            accept_poll,
            semaphore_poll,
            stop_after,
        }
    }
    fn spawn_handler(&mut self, s: TcpStream, permit: OwnedSemaphorePermit) -> JoinHandle<()> {
        tokio::spawn(async move {
            handler(s).await;
            drop(permit);
        })
    }
    fn switch_state(&mut self, to_state: State) {
        debug!(message="switching state", from=?self.state, to=?to_state);
        self.state = to_state;
    }
}

#[derive(Debug)]
enum PollRes {
    S, // Success
    E, // Error
}

async fn handler(s: TcpStream) {
    let mut s = s;
    log::debug!("handler enter");
    let _ = s
        .write_all("hello\nsleeping for 5 seconds\n".as_bytes())
        .await;
    sleep(Duration::from_secs(5)).await;
    let _ = s.write_all("closing\n".as_bytes()).await;
    log::debug!("handler leave");
}
impl<'a> fmt::Debug for MyFut<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MyFut").field("state", &self.state).finish()
    }
}
impl<'a> Future for MyFut<'a> {
    type Output = PollRes;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        if this.stop_after == 0 {
            return Poll::Ready(PollRes::S);
        }
        loop {
            match this.state {
                State::AcceptConnections => match this.accept_poll.as_mut().poll(cx) {
                    Poll::Ready(Ok((s, _))) => {
                        debug!("got listener Ready");
                        this.accept_poll = Box::pin(this.listener.accept());
                        match this.semaphore_poll.clone_inner().try_acquire_owned() {
                            Ok(permit) => {
                                this.stop_after -= 1;
                                debug!(message="try_acquire ok", available_permits=?this.semaphore_poll.available_permits());
                                this.spawn_handler(s, permit);
                            }
                            Err(e) => {
                                debug!(try_permit_err=?e);
                                log::error!("no task slots");
                                this.switch_state(State::WaitForTaskCompletion);
                            }
                        }
                        continue;
                    }
                    Poll::Ready(Err(e)) => {
                        log::error!("{:?}", e);
                        return Poll::Ready(PollRes::E);
                    }
                    Poll::Pending => {
                        debug!("got listener Pending");
                        return Poll::Pending;
                    }
                },

                State::WaitForTaskCompletion => {
                    match this.semaphore_poll.poll_acquire(cx) {
                        Poll::Ready(Some(_)) => {
                            debug!("sem    Ready   in WaitForTaskCompletion");
                            // parmit available, wait for next connection
                            this.switch_state(State::AcceptConnections);
                            continue;
                        }
                        Poll::Ready(None) => {
                            log::error!("Semaphore closed");
                            return Poll::Ready(PollRes::E);
                        }
                        Poll::Pending => {
                            debug!("sem    Pending in WaitForTaskCompletion");
                            // have to check if any connections ready for accept
                        }
                    }
                    match this.accept_poll.as_mut().poll(cx) {
                        Poll::Ready(Ok((..))) => {
                            debug!("accept Ready   in WaitForTaskCompletion - drop connection.");
                            log::error!("no task slots");
                            this.accept_poll = Box::pin(this.listener.accept());
                        }
                        Poll::Ready(Err(e)) => {
                            log::error!("{:?}", e);
                            return Poll::Ready(PollRes::E);
                        }
                        Poll::Pending => {
                            debug!("accept Pending in WaitForTaskCompletion");
                            // no permits available and no incoming connections
                            return Poll::Pending;
                        }
                    }
                }
            }
        }
    }
}

#[instrument]
async fn run(task_pool: usize, stop_after: usize) -> Result<(), Report> {
    let listener = Arc::new(TcpListener::bind("127.0.0.1:9000").await.unwrap());
    let mf = MyFut::new(&listener, task_pool, stop_after);
    let x = mf.await;
    log::info!("{:?}", x);
    drop(listener);

    let listener = TcpListener::bind("127.0.0.1:9000").await?;
    let semaphore = Arc::new(Semaphore::new(task_pool));
    let mut stop_after = stop_after;
    let mut state = State::AcceptConnections;
    while stop_after > 0 {
        let accept = listener.accept();
        let acquire = semaphore.acquire();
        if state == State::WaitForTaskCompletion {
            select! {
                _ = acquire => {
                    state = State::AcceptConnections;
                    debug!("reenable connections");
                },
                Ok((s, _a)) = accept => {
                    log::error!("drop connection");
                    drop(s);
                }
            }
        } else if state == State::AcceptConnections {
            select! {
                Ok((s, _a)) = accept => {
                    stop_after -= 1;
                    if semaphore.available_permits() == 0 {
                        log::error!("drop connection");
                        drop(s);
                        debug!("disable connections");
                        state = State::WaitForTaskCompletion;
                        continue;
                    }
                    let se  = semaphore.clone();
                    tokio::spawn(async move {
                        let permit = se.try_acquire();
                        handler(s).await;
                        drop(permit);
                    });
                }
            }
        } else {
            todo!("unknown state")
        }
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = parse_args();

    let task_pool = args.value_of("task_pool").unwrap_or("64").parse()?;
    let workers = args.value_of("workers").unwrap_or("4").parse()?;
    let stop_after = args.value_of("stop_after").unwrap_or("100").parse()?;

    if env::var("RUST_LOG").is_err() {
        env::set_var(
            "RUST_LOG",
            args.value_of("loglvl").unwrap_or("info").to_owned(),
        )
    }

    color_eyre::install()?;
    env_logger::init();

    let subscriber = tracing_subscriber::FmtSubscriber::builder()
        .with_max_level(tracing::Level::TRACE)
        .finish();
    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(workers)
        .build()?
        .block_on(run(task_pool, stop_after))?;
    Ok(())
}
