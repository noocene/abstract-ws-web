use abstract_ws::{Socket as AbstractSocket, SocketProvider, Url};

use futures::{
    channel::{
        mpsc::{unbounded, UnboundedReceiver},
        oneshot::channel,
    },
    future::select,
    Future, Sink, Stream,
};
use js_sys::Uint8Array;
use wasm_bindgen::{prelude::Closure, JsCast, JsValue};
use web_sys::{MessageEvent, WebSocket};

use core::{
    pin::Pin,
    task::{Context, Poll},
};

pub struct Socket {
    inner: WebSocket,
    #[allow(dead_code)]
    on_message: Closure<dyn FnMut(MessageEvent)>,
    #[allow(dead_code)]
    on_close: Closure<dyn FnMut(JsValue)>,
    messages: UnboundedReceiver<Vec<u8>>,
}

impl Socket {
    fn new(url: Url) -> impl Future<Output = Result<Self, JsValue>> {
        let (sender, receiver) = unbounded();

        let socket = WebSocket::new(url.as_ref());

        async move {
            let socket = socket?;

            let (open_sender, open) = channel();
            let (error_sender, error) = channel();

            let mut open_sender = Some(open_sender);
            let mut error_sender = Some(error_sender);

            let on_open = Closure::wrap(Box::new(move |_: JsValue| {
                let _ = open_sender.take().map(|sender| sender.send(None));
            }) as Box<dyn FnMut(_)>);

            let on_error = Closure::wrap(Box::new(move |e: JsValue| {
                let _ = error_sender.take().map(|sender| sender.send(Some(e)));
            }) as Box<dyn FnMut(_)>);

            socket.set_onopen(Some(on_open.as_ref().unchecked_ref()));
            socket.set_onerror(Some(on_error.as_ref().unchecked_ref()));

            let res = select(open, error).await.factor_first().0.unwrap();

            socket.set_onopen(None);
            socket.set_onerror(None);

            let on_message = {
                let sender = sender.clone();

                Closure::wrap(Box::new(move |e: MessageEvent| {
                    let buffer = Uint8Array::new(&e.data());
                    let mut data = vec![0u8; buffer.length() as usize];
                    buffer.copy_to(&mut data);
                    let _ = sender.unbounded_send(data);
                }) as Box<dyn FnMut(_)>)
            };

            socket.set_onmessage(Some(on_message.as_ref().unchecked_ref()));

            let on_close = Closure::wrap(Box::new(move |_: JsValue| {
                sender.close_channel();
            }) as Box<dyn FnMut(_)>);

            socket.set_onclose(Some(on_close.as_ref().unchecked_ref()));

            if let Some(res) = res {
                Err(res)
            } else {
                Ok(Socket {
                    messages: receiver,
                    on_message,
                    on_close,
                    inner: socket,
                })
            }
        }
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        let _ = self.inner.close();
    }
}

impl Stream for Socket {
    type Item = Vec<u8>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.messages).poll_next(cx)
    }
}

impl Sink<Vec<u8>> for Socket {
    type Error = JsValue;

    fn poll_ready(self: Pin<&mut Self>, _: &mut Context) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn start_send(self: Pin<&mut Self>, item: Vec<u8>) -> Result<(), Self::Error> {
        self.inner.send_with_u8_array(&item)
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _: &mut Context) -> Poll<Result<(), Self::Error>> {
        self.inner.close()?;

        Poll::Ready(Ok(()))
    }
}

impl AbstractSocket for Socket {}

pub struct Provider;

impl SocketProvider for Provider {
    type Socket = Socket;
    type Connect = Pin<Box<dyn Future<Output = Result<Self::Socket, JsValue>>>>;

    fn connect(&self, url: Url) -> Self::Connect {
        Box::pin(Socket::new(url))
    }
}
