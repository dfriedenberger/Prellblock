use super::{
    super::{Block, BlockHash, BlockNumber, Body, LeaderTerm},
    error::PhaseName,
    message::ConsensusMessage,
    state::{FollowerState, Phase, PhaseMeta, RoundState, ViewPhase, ViewPhaseMeta},
    Error, PRaftBFT,
};
use pinxit::{PeerId, Signable, Signature, Signed};
use prellblock_client_api::Transaction;
use std::{collections::HashMap, time::Duration};
use tokio::{
    sync::{watch, MutexGuard},
    time,
};

// After this amount of time a transaction should be committed.
const CENSORSHIP_TIMEOUT: Duration = Duration::from_secs(10);

#[allow(clippy::single_match_else)]
impl PRaftBFT {
    /// Wait until we reached the block number the message is at.
    async fn follower_state_in_block(
        &self,
        block_number: BlockNumber,
    ) -> MutexGuard<'_, FollowerState> {
        let mut receiver = self.block_changed_receiver.clone();
        loop {
            let follower_state = self.follower_state.lock().await;
            if follower_state.block_number + 1 >= block_number {
                return follower_state;
            }
            drop(follower_state);
            // Wait until block number changed.
            let _ = receiver.recv().await;
        }
    }

    async fn handle_prepare_message(
        &self,
        peer_id: &PeerId,
        leader_term: LeaderTerm,
        block_number: BlockNumber,
        block_hash: BlockHash,
    ) -> Result<ConsensusMessage, Error> {
        let mut follower_state = self.follower_state_in_block(block_number).await;
        if !self.is_current_leader(leader_term, peer_id) {
            log::warn!("Received message from invalid leader (ID: {}).", peer_id);
            return Err(Error::WrongLeader(peer_id.clone()));
        }
        follower_state.verify_message_meta(leader_term, block_number)?;

        // Check whether the state for the block is Waiting.
        // We only allow to receive messages once.
        let round_state = follower_state.round_state(block_number)?;
        if !matches!(round_state.phase, Phase::Waiting) {
            return Err(Error::WrongPhase {
                current: round_state.phase.to_phase_name(),
                expected: PhaseName::Waiting,
            });
        }

        // All checks passed, update our state.
        let leader = self.leader(follower_state.leader_term);
        follower_state.round_state_mut(block_number).unwrap().phase =
            Phase::Prepare(PhaseMeta { leader, block_hash });

        // Send AckPrepare to the leader.
        // *Note*: Technically, we only need to send a signature of
        // the PREPARE message.
        let ackprepare_message = ConsensusMessage::AckPrepare {
            leader_term: follower_state.leader_term,
            block_number,
            block_hash,
        };

        // Done :D
        Ok(ackprepare_message)
    }

    #[allow(clippy::too_many_lines)]
    async fn handle_append_message(
        &self,
        peer_id: &PeerId,
        leader_term: LeaderTerm,
        block_number: BlockNumber,
        block_hash: BlockHash,
        ackprepare_signatures: HashMap<PeerId, Signature>,
        data: Vec<Signed<Transaction>>,
    ) -> Result<ConsensusMessage, Error> {
        let mut follower_state = self.follower_state_in_block(block_number).await;
        log::trace!("Handle Append message #{}.", block_number);
        if !self.is_current_leader(leader_term, peer_id) {
            log::warn!("Received message from invalid leader (ID: {}).", peer_id);
            return Err(Error::WrongLeader(peer_id.clone()));
        }
        follower_state.verify_message_meta(leader_term, block_number)?;

        // Check whether the state for the block is Prepare.
        // We only allow to receive messages once.
        let round_state = follower_state.round_state(block_number)?;
        let meta = match &round_state.phase {
            Phase::Prepare(meta) => meta.clone(),
            Phase::Waiting => {
                let leader = self.leader(follower_state.leader_term);
                PhaseMeta { leader, block_hash }
            }
            _ => {
                return Err(Error::WrongPhase {
                    current: round_state.phase.to_phase_name(),
                    expected: PhaseName::Prepare,
                });
            }
        };

        if block_hash != meta.block_hash {
            return Err(Error::ChangedBlockHash);
        }

        if block_number != follower_state.block_number + 1 {
            return Err(Error::WrongBlockNumber(block_number));
        }

        // Check validity of ACKPREPARE Signatures.
        if !self.supermajority_reached(ackprepare_signatures.len()) {
            self.request_view_change(follower_state).await;
            return Err(Error::NotEnoughSignatures);
        }

        let ackprepare_message = ConsensusMessage::AckPrepare {
            leader_term,
            block_number,
            block_hash,
        };
        for (peer_id, signature) in ackprepare_signatures {
            // All signatures in here must be valid. The leader would filter out any
            // wrong signatures.
            match peer_id.verify(&ackprepare_message, &signature) {
                Ok(()) => {}
                Err(err) => {
                    log::error!("Error while verifying ACKPREPARE signatures: {}", err);
                    self.request_view_change(follower_state).await;
                    return Err(err.into());
                }
            };
            // Also check whether the signer is a known RPU
            self.permission_checker.verify_is_rpu(&peer_id)?;
        }

        if data.is_empty() {
            // Empty blocks are not allowed.
            // Trigger leader change as a consequence.
            self.request_view_change(follower_state).await;
            return Err(Error::EmptyBlock);
        }

        // Check for transaction validity.
        for tx in &data {
            let signer = tx.signer().clone();
            let transaction = match tx.verify_ref() {
                Ok(transaction) => transaction,
                Err(err) => {
                    log::error!("Error while verifying transaction signature: {}", err);
                    self.request_view_change(follower_state).await;
                    return Err(err.into());
                }
            };
            match self.permission_checker.verify(&signer, &transaction) {
                Ok(_) => {}
                Err(err) => {
                    log::error!("Error while verifying client account permissions: {}", err);
                    self.request_view_change(follower_state).await;
                    return Err(err.into());
                }
            };
        }

        let validated_transactions = data;

        // Validate the Block Hash.
        let body = Body {
            height: block_number,
            prev_block_hash: follower_state.last_block_hash(),
            transactions: validated_transactions,
        };
        if block_hash != body.hash() {
            return Err(Error::WrongBlockHash);
        }
        let leader_term = follower_state.leader_term;
        // All checks passed, update our state.
        let round_state_mut = follower_state.round_state_mut(block_number).unwrap();
        round_state_mut.phase = Phase::Append(meta, body);

        // There could be a commit message for this block number that arrived first.
        // We then need to apply the commit (or at least check).
        if let Some(buffered_message) = round_state_mut.buffered_commit_message.take() {
            match buffered_message {
                ConsensusMessage::Commit {
                    leader_term: buffered_leader_term,
                    block_number: buffered_block_number,
                    block_hash: buffered_block_hash,
                    ackappend_signatures: buffered_ackappend_signatures,
                } => {
                    let commit_result = self
                        .handle_commit_message_inner(
                            follower_state,
                            peer_id,
                            buffered_leader_term,
                            buffered_block_number,
                            buffered_block_hash,
                            buffered_ackappend_signatures,
                        )
                        .await;
                    match commit_result {
                        Ok(_) => log::debug!("Used out-of-order commit."),
                        Err(err) => log::debug!("Failed to apply out-of-order commit: {}", err),
                    }
                }
                _ => unreachable!(),
            }
        }

        let ackappend_message = ConsensusMessage::AckAppend {
            leader_term,
            block_number,
            block_hash,
        };
        Ok(ackappend_message)
    }

    async fn handle_commit_message(
        &self,
        peer_id: &PeerId,
        leader_term: LeaderTerm,
        block_number: BlockNumber,
        block_hash: BlockHash,
        ackappend_signatures: HashMap<PeerId, Signature>,
    ) -> Result<ConsensusMessage, Error> {
        let follower_state = self.follower_state_in_block(block_number).await;
        self.handle_commit_message_inner(
            follower_state,
            peer_id,
            leader_term,
            block_number,
            block_hash,
            ackappend_signatures,
        )
        .await
    }

    /// This function is used for out-of-order message reception and
    /// applying these commits.
    async fn handle_commit_message_inner(
        &self,
        mut follower_state: MutexGuard<'_, FollowerState>,
        peer_id: &PeerId,
        leader_term: LeaderTerm,
        block_number: BlockNumber,
        block_hash: BlockHash,
        ackappend_signatures: HashMap<PeerId, Signature>,
    ) -> Result<ConsensusMessage, Error> {
        log::trace!("Handle Commit message #{}.", block_number);
        if !self.is_current_leader(leader_term, peer_id) {
            log::warn!("Received message from invalid leader (ID: {}).", peer_id);
            return Err(Error::WrongLeader(peer_id.clone()));
        }
        follower_state.verify_message_meta(leader_term, block_number)?;

        // Check whether the state for the block is Append.
        // We only allow to receive messages once.
        let round_state = follower_state.round_state(block_number)?;
        let (meta, body) = match &round_state.phase {
            Phase::Waiting | Phase::Prepare(..) => {
                let current_phase_name = round_state.phase.to_phase_name();
                let consensus_message = ConsensusMessage::Commit {
                    leader_term,
                    block_number,
                    block_hash,
                    ackappend_signatures,
                };
                follower_state
                    .round_state_mut(block_number)
                    .unwrap()
                    .buffered_commit_message = Some(consensus_message);
                return Err(Error::WrongPhase {
                    current: current_phase_name,
                    expected: PhaseName::Append,
                });
            }
            Phase::Append(meta, body) => (meta, body.clone()),
            _ => {
                return Err(Error::WrongPhase {
                    current: round_state.phase.to_phase_name(),
                    expected: PhaseName::Append,
                });
            }
        };

        if block_hash != meta.block_hash {
            return Err(Error::ChangedBlockHash);
        }

        if block_number != follower_state.block_number + 1 {
            return Err(Error::WrongBlockNumber(block_number));
        }

        // Check validity of ACKAPPEND Signatures.
        if !self.supermajority_reached(ackappend_signatures.len()) {
            self.request_view_change(follower_state).await;
            return Err(Error::NotEnoughSignatures);
        }
        let ackappend_message = ConsensusMessage::AckAppend {
            leader_term,
            block_number,
            block_hash,
        };
        for (peer_id, signature) in &ackappend_signatures {
            // All signatures in here must be valid. The leader would filter out any
            // wrong signatures.
            match peer_id.verify(&ackappend_message, signature) {
                Ok(()) => {}
                Err(err) => {
                    log::error!("Error while verifying ACKAPPEND signatures: {}", err);
                    self.request_view_change(follower_state).await;
                    return Err(err.into());
                }
            };
            // Also check whether the signer is a known RPU
            self.permission_checker.verify_is_rpu(peer_id)?;
        }

        follower_state.round_state_mut(block_number).unwrap().phase = Phase::Committed(block_hash);

        let old_round_state = follower_state.round_states.increment(RoundState::default());
        assert!(matches!(old_round_state.phase, Phase::Committed(..)));
        assert!(old_round_state.buffered_commit_message.is_none());

        follower_state.block_number = block_number;
        let _ = self.block_changed_notifier.broadcast(());

        let block = Block {
            body,
            signatures: ackappend_signatures,
        };
        // Write Block to BlockStorage
        self.block_storage.write_block(&block).unwrap();

        // Remove committed transactions from our queue.
        self.queue
            .write()
            .await
            .retain(|(_, transaction)| !block.body.transactions.contains(transaction));

        // Write Block to WorldState
        let mut world_state = self.world_state.get_writable().await;
        world_state.apply_block(block).unwrap();
        world_state.save();

        log::debug!(
            "Committed block #{} with hash {:?}.",
            block_number,
            block_hash
        );
        Ok(ConsensusMessage::AckCommit)
    }

    /// Process the incoming `ConsensusMessages` (`PREPARE`, `ACKPREPARE`, `APPEND`, `ACKAPPEND`, `COMMIT`).
    pub async fn handle_message(
        &self,
        message: Signed<ConsensusMessage>,
    ) -> Result<Signed<ConsensusMessage>, Error> {
        // Only RPUs are allowed.
        if !self.peer_ids().any(|peer_id| *message.signer() == peer_id) {
            return Err(Error::InvalidPeer(message.signer().clone()));
        }

        let message = message.verify()?;
        let peer_id = message.signer().clone();
        let signature = message.signature().clone();

        let response = match message.into_inner() {
            ConsensusMessage::Prepare {
                leader_term,
                block_number,
                block_hash,
            } => {
                self.handle_prepare_message(&peer_id, leader_term, block_number, block_hash)
                    .await?
            }
            ConsensusMessage::Append {
                leader_term,
                block_number,
                block_hash,
                ackprepare_signatures,
                data,
            } => {
                self.handle_append_message(
                    &peer_id,
                    leader_term,
                    block_number,
                    block_hash,
                    ackprepare_signatures,
                    data,
                )
                .await?
            }
            ConsensusMessage::Commit {
                leader_term,
                block_number,
                block_hash,
                ackappend_signatures,
            } => {
                self.handle_commit_message(
                    &peer_id,
                    leader_term,
                    block_number,
                    block_hash,
                    ackappend_signatures,
                )
                .await?
            }
            ConsensusMessage::ViewChange { new_leader_term } => {
                self.handle_view_change(peer_id, signature, new_leader_term)
                    .await?
            }
            ConsensusMessage::NewView {
                leader_term,
                view_change_signatures,
            } => {
                self.handle_new_view(&peer_id, leader_term, view_change_signatures)
                    .await?
            }
            _ => unimplemented!(),
        };

        let signed_response = response.sign(&self.broadcast_meta.identity).unwrap();
        Ok(signed_response)
    }

    /// Send a `ConsensusMessage::ViewChange` message because the leader
    /// seems to be faulty.
    async fn request_view_change(&self, mut follower_state: MutexGuard<'_, FollowerState>) {
        let requested_new_leader_term = follower_state.leader_term + 1;
        let messages = HashMap::new();
        match follower_state.set_view_phase(
            requested_new_leader_term,
            ViewPhase::ViewChanging(ViewPhaseMeta { messages }),
        ) {
            Ok(()) => {}
            Err(err) => log::error!("Error setting view change phase: {}", err),
        };
        // This drop is needed because receiving messages while
        // broadcasting also requires the lock.
        drop(follower_state);

        match self.broadcast_view_change(requested_new_leader_term).await {
            Ok(()) => {}
            Err(err) => log::error!("Error broadcasting ViewChange: {}", err),
        }
    }

    /// This is woken up after a timeout or a specific
    /// number of blocks commited.
    pub(super) async fn censorship_checker(
        &self,
        mut new_view_receiver: watch::Receiver<LeaderTerm>,
    ) {
        loop {
            let timeout_result = time::timeout(CENSORSHIP_TIMEOUT, new_view_receiver.recv()).await;
            // If there was no timeout, a leader change happened.
            // Give the leader enough time by sleeping again.
            if timeout_result.is_ok() {
                continue;
            }

            let queue = self.queue.read().await;
            // Iterating over the queue should be pretty fast.
            // If there are no old transactions, we should only have
            // a few transactions to iterate over.
            let has_old_transactions = queue
                .iter()
                .any(|(timestamp, _)| timestamp.elapsed() > CENSORSHIP_TIMEOUT);
            drop(queue);

            if has_old_transactions {
                // leader seems to be faulty / dead or censoring
                let follower_state = self.follower_state.lock().await;
                let leader = self.leader(follower_state.leader_term);
                log::warn!(
                    "Found censored transactions from leader {}. Requesting View Change.",
                    leader
                );
                self.request_view_change(follower_state).await;
            } else {
                log::trace!("No old transactions found while checking for censorship.");
            }
        }
    }
}
