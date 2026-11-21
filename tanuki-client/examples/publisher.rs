//! Publishes the laptop battery as a 120-second lease.
use std::time::Duration;
use tanuki_client::{ClientError, Expiry, Session, StateTopic};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), ClientError> {
    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "ws://127.0.0.1:5167/v1/ws".into());
    let session = Session::connect(&url, "example laptop").await?;
    let battery = StateTopic::<u8>::new("/battery/laptop")?;
    let receipt = session
        .publish_with(&battery, &82, Expiry::After(Duration::from_secs(120)))
        .await?;
    println!("accepted battery=82 at sequence {}", receipt.sequence);
    session.close().await;
    Ok(())
}
