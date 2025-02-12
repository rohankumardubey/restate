// Copyright (c) 2023 -  Restate Software, Inc., Restate GmbH.
// All rights reserved.
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of the Change Date specified in that file, in accordance with
// the Business Source License, use of this software will be governed
// by the Apache License, Version 2.0.

use restate_errors::NotRunningError;
use restate_types::identifiers::{PartitionId, PartitionKey, PeerId};
use std::fmt::Debug;
use std::future::Future;
use tokio::sync::mpsc;

mod routing;
mod unbounded_handle;

pub use routing::{Network, PartitionProcessorSender, RoutingError};
pub use unbounded_handle::UnboundedNetworkHandle;

pub type ShuffleSender<T> = mpsc::Sender<T>;

/// Handle to interact with the running network routing component.
pub trait NetworkHandle<ShuffleIn, ShuffleOut> {
    type Future: Future<Output = Result<(), NotRunningError>>;

    fn register_shuffle(
        &self,
        peer_id: PeerId,
        shuffle_sender: mpsc::Sender<ShuffleIn>,
    ) -> Self::Future;

    fn unregister_shuffle(&self, peer_id: PeerId) -> Self::Future;

    fn create_shuffle_sender(&self) -> ShuffleSender<ShuffleOut>;
}

enum NetworkCommand<ShuffleIn> {
    RegisterShuffle {
        peer_id: PeerId,
        shuffle_tx: mpsc::Sender<ShuffleIn>,
    },
    UnregisterShuffle {
        peer_id: PeerId,
    },
}

/// Trait for messages that are sent to the shuffle component
pub trait TargetShuffle {
    /// Returns the target shuffle identified by its [`PeerId`].
    fn shuffle_target(&self) -> PeerId;
}

pub enum ConsensusOrIngressTarget<C, I> {
    Consensus(C),
    Ingress(I),
}

/// Trait for messages that are sent to the consensus module or an ingress
pub trait TargetConsensusOrIngress<C, I> {
    /// Returns the target of a message. It can either be an ingress
    /// or the consensus module.
    fn into_target(self) -> ConsensusOrIngressTarget<C, I>;
}

pub enum ConsensusOrShuffleTarget<C, S> {
    Consensus(C),
    Shuffle(S),
}

/// Trait for messages that are sent to the consensus module or a shuffle
pub trait TargetConsensusOrShuffle<C, S> {
    /// Returns the target of a message. It can either be the consensus module
    /// or a shuffle
    fn into_target(self) -> ConsensusOrShuffleTarget<C, S>;
}

/// Trait for messages that are sent to a shuffle component or an ingress
pub enum ShuffleOrIngressTarget<S, I> {
    Shuffle(S),
    Ingress(I),
}

pub trait TargetShuffleOrIngress<S, I> {
    /// Returns the target of a message. It can either be a shuffle or an ingress.
    fn into_target(self) -> ShuffleOrIngressTarget<S, I>;
}

#[derive(Debug, thiserror::Error)]
#[error("Cannot find target peer for partition key {0}")]
pub struct PartitionTableError(PartitionKey);

pub trait FindPartition {
    fn find_partition_id(
        &self,
        partition_key: PartitionKey,
    ) -> Result<PartitionId, PartitionTableError>;
}
