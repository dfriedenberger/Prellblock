//! A server for communicating between RPUs.

use super::{PeerInbox, PeerMessage};
use balise::{
    handler,
    server::{Server, TlsIdentity},
};

use std::sync::Arc;
use connection::listener::Listener;

/// A receiver (server) instance.
///
/// The `Receiver` is used to receive messages being sent between RPUs.
#[derive(Clone)]
pub struct Receiver {
    tls_identity: TlsIdentity,
    peer_inbox: Arc<PeerInbox>,
}

impl Receiver {
    /// Create a new receiver instance.
    #[must_use]
    pub const fn new(tls_identity: TlsIdentity, peer_inbox: Arc<PeerInbox>) -> Self {
        Self {
            tls_identity,
            peer_inbox,
        }
    }

    /// The main server loop.
    pub async fn serve(self, listener: &mut dyn Listener) -> Result<(), balise::Error> {
        let tls_identity = self.tls_identity.clone();
        let server = Server::new(
            handler!(PeerMessage, {
                Add(params) =>  self.peer_inbox.handle_add(&params),
                Sub(params) =>  self.peer_inbox.handle_sub(&params),
                Ping(_) => self.peer_inbox.handle_ping(),
                ExecuteBatch(params) => self.peer_inbox.handle_execute_batch(params).await,
                Consensus(params) => self.peer_inbox.handle_consensus(params).await,
            }),
            tls_identity,
        )?;
        server.serve(listener).await?;
        Ok(())
    }
}
