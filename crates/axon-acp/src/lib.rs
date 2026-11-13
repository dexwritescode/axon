//! ACP (Agent Client Protocol) frontend: serves an Axon agent over stdio.

use agent_client_protocol::schema::v1::{AgentCapabilities, InitializeRequest, InitializeResponse};
use agent_client_protocol::{Agent, Result, Stdio};

/// Run the ACP agent on stdin/stdout until the client disconnects.
pub async fn serve() -> Result<()> {
    Agent
        .builder()
        .name("Axon")
        .on_receive_request(
            async move |initialize: InitializeRequest, responder, _connection| {
                responder.respond(
                    InitializeResponse::new(initialize.protocol_version)
                        .agent_capabilities(AgentCapabilities::new()),
                )
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_to(Stdio::new())
        .await
}
