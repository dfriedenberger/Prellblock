//! A server for communicating between RPUs.

use std::{
    convert::TryInto,
    io::{self, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    time::Duration,
};

use super::{client, Ping, Pong, Request, RequestData};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// A server instance.
#[derive(Clone)]
pub struct Server {}

impl Server {
    /// The main server loop.
    pub fn serve(self, listener: TcpListener) -> Result<(), BoxError> {
        log::info!(
            "Server is now listening on Port {}",
            listener.local_addr()?.port()
        );
        for stream in listener.incoming() {
            // TODO: Is there a case where we should continue to listen for incoming streams?
            let stream = stream?;

            let clone_self = self.clone();

            // handle the client in a new thread
            std::thread::spawn(move || {
                let peer_addr = stream.peer_addr().unwrap();
                log::info!("Connected: {}", peer_addr);
                match clone_self.handle_client(stream) {
                    Ok(()) => log::info!("Disconnected"),
                    Err(err) => log::warn!("Server error: {:?}", err),
                }
            });
        }
        Ok(())
    }

    fn handle_client(self, mut stream: TcpStream) -> Result<(), BoxError> {
        let addr = stream.peer_addr().expect("Peer address");
        loop {
            // read message length
            let mut len_buf = [0; 4];
            match stream.read_exact(&mut len_buf) {
                Ok(()) => {}
                Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => break,
                Err(err) => return Err(err.into()),
            };

            let len = u32::from_le_bytes(len_buf) as usize;

            // read message
            let mut buf = vec![0; len];
            stream.read_exact(&mut buf)?;

            // handle the request
            let res = match self.handle_request(&addr, buf) {
                Ok(res) => Ok(res),
                Err(err) => Err(err.to_string()),
            };

            // serialize response
            let data = serde_json::to_vec(&res)?;

            // send response
            let size: u32 = data.len().try_into()?;
            let size = size.to_le_bytes();
            stream.write(&size)?;
            stream.write_all(&data)?;
        }
        Ok(())
    }

    fn handle_request(
        &self,
        addr: &SocketAddr,
        req: Vec<u8>,
    ) -> Result<serde_json::Value, BoxError> {
        // Deserialize request.
        let req: RequestData = serde_json::from_slice(&req)?;
        log::trace!("Received request from {}: {:?}", addr, req);
        // handle the actual request
        let res = match req {
            RequestData::Add(params) => params.handle(|params| params.0 + params.1),
            RequestData::Sub(params) => params.handle(|params| params.0 - params.1),
            RequestData::Ping(params) => params.handle(|_| {
                let mut addr = addr.clone();
                addr.set_port(2480);
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(100));
                    let mut client = client::Client::new(addr);
                    client.send_request(Ping());
                    client.send_request(Ping());
                    client.send_request(Ping());
                    client.send_request(Ping());
                });
                Pong
            }),
        };
        log::trace!("Send response to {}: {:?}", addr, res);
        Ok(res?)
    }
}

trait ServerRequest: Request + Sized {
    fn handle(
        self,
        handler: impl FnOnce(Self) -> Self::Response,
    ) -> Result<serde_json::Value, BoxError> {
        let res = handler(self);
        Ok(serde_json::to_value(&res)?)
    }
}

impl<T> ServerRequest for T where T: Request {}
