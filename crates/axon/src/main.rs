#[tokio::main]
async fn main() -> agent_client_protocol::Result<()> {
    axon_acp::serve().await
}
