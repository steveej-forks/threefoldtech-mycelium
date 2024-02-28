use std::{
    net::SocketAddr,
    str::FromStr,
    sync::{Arc, Mutex},
};

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get},
    Json, Router,
};
use log::{debug, error};
use serde::{Deserialize, Serialize};

#[cfg(feature = "message")]
use crate::message::MessageStack;
use crate::{
    endpoint::Endpoint,
    peer_manager::{PeerExists, PeerManager, PeerNotFound, PeerStats},
};

#[cfg(feature = "message")]
mod message;
#[cfg(feature = "message")]
pub use message::{MessageDestination, MessageReceiveInfo, MessageSendInfo, PushMessageResponse};

/// Http API server handle. The server is spawned in a background task. If this handle is dropped,
/// the server is terminated.
pub struct Http {
    /// Channel to send cancellation to the http api server. We just keep a reference to it since
    /// dropping it will also cancel the receiver and thus the server.
    _cancel_tx: tokio::sync::oneshot::Sender<()>,
}

#[derive(Clone)]
/// Shared state accessible in HTTP endpoint handlers.
struct HttpServerState {
    /// Access to the (`Router`)(crate::router::Router) state. This is only meant as read only view.
    router: Arc<Mutex<crate::router::Router>>,
    /// Access to the connection state of (`Peer`)[crate::peer::Peer]s.
    peer_manager: PeerManager,
    #[cfg(feature = "message")]
    /// Access to messages.
    message_stack: MessageStack,
}

impl Http {
    /// Spawns a new HTTP API server on the provided listening address.
    pub fn spawn(
        router: crate::router::Router,
        peer_manager: PeerManager,
        #[cfg(feature = "message")] message_stack: MessageStack,
        listen_addr: &SocketAddr,
    ) -> Self {
        let server_state = HttpServerState {
            router: Arc::new(Mutex::new(router)),
            peer_manager,
            #[cfg(feature = "message")]
            message_stack,
        };
        let admin_routes = Router::new()
            .route("/admin", get(get_info))
            .route("/admin/peers", get(get_peers).post(add_peer))
            .route("/admin/peers/:endpoint", delete(delete_peer))
            .route("/admin/routes/selected", get(get_selected_routes))
            .route("/admin/routes/fallback", get(get_fallback_routes))
            .with_state(server_state.clone());
        let mut app = Router::new();
        app = app.nest("/api/v1", admin_routes);
        #[cfg(feature = "message")]
        {
            app = app.nest("/api/v1", message::message_router_v1(server_state));
        }

        let (_cancel_tx, cancel_rx) = tokio::sync::oneshot::channel();
        let server = axum::Server::bind(listen_addr)
            .serve(app.into_make_service())
            .with_graceful_shutdown(async {
                cancel_rx.await.ok();
            });

        tokio::spawn(async {
            if let Err(e) = server.await {
                error!("Http API server error: {e}");
            }
        });
        Http { _cancel_tx }
    }
}

/// Get the stats of the current known peers
async fn get_peers(State(state): State<HttpServerState>) -> Json<Vec<PeerStats>> {
    debug!("Fetching peer stats");
    Json(state.peer_manager.peers())
}

/// Payload of an add_peer request
#[derive(Deserialize)]
pub struct AddPeer {
    /// The endpoint used to connect to the peer
    pub endpoint: String,
}

/// Add a new peer to the system
async fn add_peer(
    State(state): State<HttpServerState>,
    Json(payload): Json<AddPeer>,
) -> Result<StatusCode, (StatusCode, String)> {
    debug!("Attempting to add peer {} to  the system", payload.endpoint);
    let endpoint = match Endpoint::from_str(&payload.endpoint) {
        Ok(endpoint) => endpoint,
        Err(e) => return Err((StatusCode::BAD_REQUEST, e.to_string())),
    };

    match state.peer_manager.add_peer(endpoint) {
        Ok(()) => Ok(StatusCode::NO_CONTENT),
        Err(PeerExists) => Err((
            StatusCode::CONFLICT,
            "A peer identified by that endpoint already exists".to_string(),
        )),
    }
}

/// remove an existing peer from the system
async fn delete_peer(
    State(state): State<HttpServerState>,
    Path(endpoint): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    debug!("Attempting to remove peer {} to  the system", endpoint);
    let endpoint = match Endpoint::from_str(&endpoint) {
        Ok(endpoint) => endpoint,
        Err(e) => return Err((StatusCode::BAD_REQUEST, e.to_string())),
    };

    match state.peer_manager.delete_peer(&endpoint) {
        Ok(()) => Ok(StatusCode::NO_CONTENT),
        Err(PeerNotFound) => Err((
            StatusCode::NOT_FOUND,
            "A peer identified by that endpoint does not exist".to_string(),
        )),
    }
}

/// Alias to a [`Metric`](crate::metric::Metric) for serialization in the API.
pub enum Metric {
    /// Finite metric
    Value(u16),
    /// Infinite metric
    Infinite,
}

/// Info about a route. This uses base types only to avoid having to introduce too many Serialize
/// bounds in the core types.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Route {
    /// We convert the [`subnet`](Subnet) to a string to avoid introducing a bound on the actual
    /// type.
    pub subnet: String,
    /// Next hop of the route, in the underlay.
    pub next_hop: String,
    /// Computed metric of the route.
    pub metric: Metric,
    /// Sequence number of the route.
    pub seqno: u16,
}

/// List all currently selected routes.
async fn get_selected_routes(State(state): State<HttpServerState>) -> Json<Vec<Route>> {
    debug!("Loading selected routes");
    let routes = state
        .router
        .lock()
        .unwrap()
        .load_selected_routes()
        .into_iter()
        .map(|sr| Route {
            subnet: sr.source().subnet().to_string(),
            next_hop: sr.neighbour().connection_identifier().clone(),
            metric: if sr.metric().is_infinite() {
                Metric::Infinite
            } else {
                Metric::Value(sr.metric().into())
            },
            seqno: sr.seqno().into(),
        })
        .collect();

    Json(routes)
}

/// List all active fallback routes.
async fn get_fallback_routes(State(state): State<HttpServerState>) -> Json<Vec<Route>> {
    debug!("Loading fallback routes");
    let routes = state
        .router
        .lock()
        .unwrap()
        .load_fallback_routes()
        .into_iter()
        .map(|sr| Route {
            subnet: sr.source().subnet().to_string(),
            next_hop: sr.neighbour().connection_identifier().clone(),
            metric: if sr.metric().is_infinite() {
                Metric::Infinite
            } else {
                Metric::Value(sr.metric().into())
            },
            seqno: sr.seqno().into(),
        })
        .collect();

    Json(routes)
}

/// General info about a node.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Info {
    /// The overlay subnet in use by the node.
    pub node_subnet: String,
}

/// Get general info about the node.
async fn get_info(State(state): State<HttpServerState>) -> Json<Info> {
    Json(Info {
        node_subnet: state.router.lock().unwrap().node_tun_subnet().to_string(),
    })
}

impl Serialize for Metric {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Infinite => serializer.serialize_str("infinite"),
            Self::Value(v) => serializer.serialize_u16(*v),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn finite_metric_serialization() {
        let metric = super::Metric::Value(10);
        let s = serde_json::to_string(&metric).expect("can encode finite metric");

        assert_eq!("10", s);
    }

    #[test]
    fn infinite_metric_serialization() {
        let metric = super::Metric::Infinite;
        let s = serde_json::to_string(&metric).expect("can encode infinite metric");

        assert_eq!("\"infinite\"", s);
    }
}
