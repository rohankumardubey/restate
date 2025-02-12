// Copyright (c) 2024 -  Restate Software, Inc., Restate GmbH.
// All rights reserved.
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of the Change Date specified in that file, in accordance with
// the Business Source License, use of this software will be governed
// by the Apache License, Version 2.0.

mod options;
mod roles;
mod server;

use codederror::CodedError;
use futures::TryFutureExt;
use restate_types::{NodeId, PlainNodeId};
use std::time::Duration;
use tokio::net::UnixStream;
use tokio::task::{JoinError, JoinSet};
use tonic::transport::{Channel, Endpoint, Uri};
use tower::service_fn;
use tracing::{info, instrument, warn};

use crate::roles::{ClusterControllerRole, WorkerRole};
use crate::server::NodeServer;
pub use options::{Options, OptionsBuilder as NodeOptionsBuilder};
pub use restate_admin::OptionsBuilder as AdminOptionsBuilder;
pub use restate_meta::OptionsBuilder as MetaOptionsBuilder;
use restate_node_services::cluster_controller::cluster_controller_client::ClusterControllerClient;
use restate_node_services::cluster_controller::AttachmentRequest;
use restate_types::nodes_config::{NetworkAddress, Role};
use restate_types::retries::RetryPolicy;
pub use restate_worker::{OptionsBuilder as WorkerOptionsBuilder, RocksdbOptionsBuilder};

#[derive(Debug, thiserror::Error, CodedError)]
pub enum Error {
    #[error("worker failed: {0}")]
    Worker(
        #[from]
        #[code]
        roles::WorkerRoleError,
    ),
    #[error("controller failed: {0}")]
    Controller(
        #[from]
        #[code]
        roles::ClusterControllerRoleError,
    ),
    #[error("node ctrl service failed: {0}")]
    NodeCtrlService(
        #[from]
        #[code]
        server::Error,
    ),
    #[error("invalid cluster controller address: {0}")]
    #[code(unknown)]
    InvalidClusterControllerAddress(http::Error),
    #[error("failed to attach to cluster at '{0}': {1}")]
    #[code(unknown)]
    Attachment(NetworkAddress, tonic::Status),
    #[error("node component panicked: {0}")]
    #[code(unknown)]
    ComponentPanic(JoinError),
}

#[derive(Debug, thiserror::Error, CodedError)]
pub enum BuildError {
    #[error("building worker failed: {0}")]
    Worker(
        #[from]
        #[code]
        roles::WorkerRoleBuildError,
    ),
    #[error("node neither runs cluster controller nor its address has been configured")]
    #[code(unknown)]
    UnknownClusterController,
}

pub struct Node {
    node_id: PlainNodeId,
    cluster_controller_address: NetworkAddress,

    cluster_controller_role: Option<ClusterControllerRole>,
    worker_role: Option<WorkerRole>,
    server: NodeServer,
}

impl Node {
    pub fn new(options: Options) -> Result<Self, BuildError> {
        let cluster_controller_role = if options.roles.contains(Role::ClusterController) {
            Some(ClusterControllerRole::try_from(options.clone()).expect("should be infallible"))
        } else {
            None
        };

        let worker_role = if options.roles.contains(Role::Worker) {
            Some(WorkerRole::try_from(options.clone())?)
        } else {
            None
        };

        let server = options.server.build(
            worker_role
                .as_ref()
                .map(|worker| (worker.rocksdb_storage().clone(), worker.bifrost_handle())),
            cluster_controller_role
                .as_ref()
                .map(|cluster_controller| cluster_controller.handle()),
        );

        let cluster_controller_address = if let Some(cluster_controller_address) =
            options.cluster_controller_address
        {
            if cluster_controller_role.is_some() {
                warn!("This node is running the cluster controller but has also a remote cluster controller address configured. \
                This indicates a potential misconfiguration. Trying to connect to the remote cluster controller.");
            }

            cluster_controller_address
        } else if cluster_controller_role.is_some() {
            NetworkAddress::DnsName(format!("127.0.0.1:{}", server.port()))
        } else {
            return Err(BuildError::UnknownClusterController);
        };

        Ok(Node {
            node_id: options.node_id,
            cluster_controller_address,
            cluster_controller_role,
            worker_role,
            server,
        })
    }

    #[instrument(level = "debug", skip_all, fields(node.id = %self.node_id))]
    pub async fn run(self, shutdown_watch: drain::Watch) -> Result<(), Error> {
        let shutdown_signal = shutdown_watch.signaled();
        tokio::pin!(shutdown_signal);

        let (component_shutdown_signal, component_shutdown_watch) = drain::channel();

        let mut component_set: JoinSet<Result<&'static str, Error>> = JoinSet::new();

        component_set.spawn(
            self.server
                .run(component_shutdown_watch.clone())
                .map_ok(|_| "server")
                .map_err(Error::NodeCtrlService),
        );

        if let Some(cluster_controller_role) = self.cluster_controller_role {
            component_set.spawn(
                cluster_controller_role
                    .run(component_shutdown_watch.clone())
                    .map_ok(|_| "cluster-controller-role")
                    .map_err(Error::Controller),
            );
        }

        tokio::select! {
            _ = &mut shutdown_signal => {
                drop(component_shutdown_watch);
                component_shutdown_signal.drain().await;
                component_set.shutdown().await;
                return Ok(());
            },
            Some(component_result) = component_set.join_next() => {
                let component_name = component_result.map_err(Error::ComponentPanic)??;
                panic!("Unexpected termination of '{component_name}'");
            }
            attachment_result = Self::attach_node(self.node_id, self.cluster_controller_address) => {
                attachment_result?
            }
        }

        if let Some(worker_role) = self.worker_role {
            component_set.spawn(
                worker_role
                    .run(component_shutdown_watch)
                    .map_ok(|_| "worker-role")
                    .map_err(Error::Worker),
            );
        } else {
            drop(component_shutdown_watch);
        }

        tokio::select! {
            _ = shutdown_signal => {
                info!("Shutting node down");
                component_shutdown_signal.drain().await;
                component_set.shutdown().await;
            },
            Some(component_result) = component_set.join_next() => {
                let component_name = component_result.map_err(Error::ComponentPanic)??;
                panic!("Unexpected termination of '{component_name}'");
            }
        }

        Ok(())
    }

    async fn attach_node(
        node_id: PlainNodeId,
        cluster_controller_address: NetworkAddress,
    ) -> Result<(), Error> {
        info!("Attach to cluster controller at '{cluster_controller_address}'");

        let channel = Self::create_channel_from_network_address(&cluster_controller_address)
            .map_err(Error::InvalidClusterControllerAddress)?;

        let cc_client = ClusterControllerClient::new(channel);

        let node_id = NodeId::from(node_id);
        RetryPolicy::exponential(Duration::from_millis(50), 2.0, 10, None)
            .retry_operation(|| async {
                cc_client
                    .clone()
                    .attach_node(AttachmentRequest {
                        node_id: Some(node_id.into()),
                    })
                    .await
            })
            .await
            .map_err(|err| Error::Attachment(cluster_controller_address, err))?;

        Ok(())
    }

    fn create_channel_from_network_address(
        cluster_controller_address: &NetworkAddress,
    ) -> Result<Channel, http::Error> {
        let channel = match cluster_controller_address {
            NetworkAddress::Uds(uds_path) => {
                let uds_path = uds_path.clone();
                // dummy endpoint required to specify an uds connector, it is not used anywhere
                Endpoint::try_from("/")
                    .expect("/ should be a valid Uri")
                    .connect_with_connector_lazy(service_fn(move |_: Uri| {
                        UnixStream::connect(uds_path.clone())
                    }))
            }
            NetworkAddress::TcpSocketAddr(socket_addr) => {
                let uri = Self::create_uri(socket_addr)?;
                Self::create_lazy_channel_from_uri(uri)
            }
            NetworkAddress::DnsName(dns_name) => {
                let uri = Self::create_uri(dns_name)?;
                Self::create_lazy_channel_from_uri(uri)
            }
        };
        Ok(channel)
    }

    fn create_uri(authority: impl ToString) -> Result<Uri, http::Error> {
        Uri::builder()
            // todo: Make the scheme configurable
            .scheme("http")
            .authority(authority.to_string())
            .path_and_query("/")
            .build()
    }

    fn create_lazy_channel_from_uri(uri: Uri) -> Channel {
        // todo: Make the channel settings configurable
        Channel::builder(uri)
            .connect_timeout(Duration::from_secs(5))
            .connect_lazy()
    }
}
