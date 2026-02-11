use core::convert::Infallible;
use std::sync::Arc;

use crate::{PublishEvent, Result, TanukiConnection};

pub struct Listener<'handler> {
    conn: Arc<TanukiConnection>,
    #[expect(clippy::type_complexity)] // oh no a boxed function
    handlers: Vec<Box<dyn FnMut(&PublishEvent) + Send + 'handler>>,
}

impl<'handler> Listener<'handler> {
    pub(super) fn new(conn: Arc<TanukiConnection>) -> Self {
        Self { conn, handlers: Vec::new() }
    }

    pub fn handle<E: for<'event> TryFrom<&'event PublishEvent, Error = ()> + Send>(
        mut self,
        mut handler: impl EventHandler<E> + Send + 'handler,
    ) -> Self {
        self.handlers.push(Box::new(move |event| {
            if let Ok(event) = E::try_from(event) {
                handler.handle(event);
            }
        }));
        self
    }

    fn dispatch(&mut self, event: &PublishEvent) {
        for handler in &mut self.handlers {
            handler(event);
        }
    }

    pub async fn listen(mut self) -> Result<Infallible> {
        loop {
            let ev = self.conn.recv().await?;
            self.dispatch(&ev);
        }
    }
}

pub trait EventHandler<E: for<'event> TryFrom<&'event PublishEvent, Error = ()>> {
    fn handle(&mut self, event: E);
}

const fn assert_send<T: Send>() {}
const _: () = assert_send::<Listener<'_>>();
