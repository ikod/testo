use color_eyre::Report;
use futures::{
    task::{Context, Poll},
    Future,
};
use log;
use std::env;
use std::{
    error::Error, fmt, io, net::SocketAddr, pin::Pin, result::Result, sync::Arc, time::Duration,
};
use testo::parse_args;
use tokio::{
    io::AsyncWriteExt,
    net::{TcpListener, TcpStream},
    sync::{OwnedSemaphorePermit, Semaphore},
    task::JoinHandle,
    time::sleep,
};
use tokio_util::sync::PollSemaphore;
use tracing::debug;

#[derive(Debug)]
enum State {
    AcceptConnections,
    WaitForTaskCompletion,
}

struct MyFut<'a> {
    state: State,
    listener: &'a TcpListener,
    accept_poll: Pin<Box<dyn Future<Output = io::Result<(TcpStream, SocketAddr)>> + 'a>>,
    semaphore_poll: PollSemaphore,
}

impl<'a> MyFut<'a> {
    fn new(listener: &'a TcpListener, pool_size: usize) -> Self {
        let accept_poll = Box::pin(listener.accept());
        let semaphore = Arc::new(Semaphore::new(pool_size));
        let semaphore_poll = PollSemaphore::new(semaphore.clone());
        MyFut {
            state: State::AcceptConnections,
            listener,
            accept_poll,
            semaphore_poll,
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
    E,
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
        loop {
            match this.state {
                State::AcceptConnections => match this.accept_poll.as_mut().poll(cx) {
                    Poll::Ready(Ok((s, _))) => {
                        debug!("got listener Ready");
                        this.accept_poll = Box::pin(this.listener.accept());
                        match this.semaphore_poll.clone_inner().try_acquire_owned() {
                            Ok(permit) => {
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

async fn run(task_pool: usize) -> Result<(), Report> {
    let listener = Arc::new(TcpListener::bind("127.0.0.1:9000").await.unwrap());
    let mf = MyFut::new(&listener, task_pool);
    let x = mf.await;
    log::info!("{:?}", x);
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = parse_args();
    let task_pool = args.value_of("task_pool").unwrap_or("64").parse()?;
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
        .worker_threads(2)
        .build()?
        .block_on(run(task_pool))?;
    Ok(())
}
