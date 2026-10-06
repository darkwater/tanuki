use tanuki_client::protocol::{ClientName, NonNegativeDuration};
use tanuki_client::{ExpiryUpdate, Session};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), tanuki_client::ClientError> {
    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "ws://127.0.0.1:5167/v1/ws".into());
    let session = Session::connect(
        &url,
        ClientName::parse("example laptop").expect("constant valid name"),
        vec![],
    )
    .await?;
    let battery = session.state::<u8>("/battery/laptop")?;
    let expiry = NonNegativeDuration::new(jiff::SignedDuration::from_secs(120))
        .expect("positive fixture duration");
    let receipt = battery.publish(&82, ExpiryUpdate::Set(expiry)).await?;
    println!(
        "accepted battery=82 at sequence {}; warnings: {:?}",
        receipt.sequence, receipt.warnings
    );
    session.close().await
}
