//! Cancel stalled socket I/O after the graceful shutdown drain window.
use axum::serve::Listener;
use std::{
    future::Future,
    io,
    net::SocketAddr,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::{TcpListener, TcpStream},
    sync::watch,
};

pub(super) struct ShutdownListener {
    listener: TcpListener,
    forced: watch::Receiver<bool>,
}

impl ShutdownListener {
    pub(super) fn new(listener: TcpListener, forced: watch::Receiver<bool>) -> Self {
        Self { listener, forced }
    }
}

impl Listener for ShutdownListener {
    type Io = ShutdownIo;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        // Axum's native listener handles accept errors and retry backoff.
        let (socket, address) = Listener::accept(&mut self.listener).await;
        let mut forced = self.forced.clone();
        let stopped = Box::pin(async move {
            let _ = forced.wait_for(|forced| *forced).await;
        });
        (
            ShutdownIo {
                socket,
                stopped: Some(stopped),
            },
            address,
        )
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.listener.local_addr()
    }
}

pub(super) struct ShutdownIo {
    socket: TcpStream,
    // A single erased notification future supplies wakeups to Hyper's I/O task.
    // Closing its sender also cancels sockets when the serving future is dropped.
    stopped: Option<Pin<Box<dyn Future<Output = ()> + Send>>>,
}

impl ShutdownIo {
    fn check_shutdown(&mut self, context: &mut Context<'_>) -> io::Result<()> {
        if self
            .stopped
            .as_mut()
            .is_none_or(|stopped| stopped.as_mut().poll(context).is_ready())
        {
            self.stopped = None;
            Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "server shutdown drain window ended",
            ))
        } else {
            Ok(())
        }
    }
}

impl AsyncRead for ShutdownIo {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        self.check_shutdown(context)?;
        Pin::new(&mut self.socket).poll_read(context, buffer)
    }
}

impl AsyncWrite for ShutdownIo {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.check_shutdown(context)?;
        Pin::new(&mut self.socket).poll_write(context, buffer)
    }

    fn poll_flush(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.check_shutdown(context)?;
        Pin::new(&mut self.socket).poll_flush(context)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.socket).poll_shutdown(context)
    }
}
