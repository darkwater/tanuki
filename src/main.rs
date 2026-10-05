#[tokio::main]
async fn main() -> std::io::Result<()> {
    tanuki::server::run_until(tokio::signal::ctrl_c()).await
}
