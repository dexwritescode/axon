#[tokio::main]
async fn main() -> anyhow::Result<()> {
    axon::tui::run().await
}
