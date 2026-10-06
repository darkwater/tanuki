use tanuki_client::protocol::{ClientName, Selector};
use tanuki_client::{ClaimRelease, ClientError, ExpiryUpdate, Session};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), ClientError> {
    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "ws://127.0.0.1:5167/v1/ws".into());
    let session = Session::connect(
        &url,
        ClientName::parse("example lamp controller").expect("constant valid name"),
        vec![Selector::parse("/lamp/desired-brightness").unwrap()],
    )
    .await?;
    let desired = session.desired::<u8>("/lamp/desired-brightness")?;
    let hue = session.state::<u16>("/lamp/hue")?;
    let brightness = session.state::<u8>("/lamp/brightness")?;
    let mut setup = session.batch();
    setup
        .define(&desired)?
        .claim(&desired, ClaimRelease::Immediate)?;
    setup.commit().await?;
    let mut intent = session.observe(&desired).await?;
    loop {
        tokio::select! {
            snapshot = intent.recv() => match snapshot? {
                Some(snapshot) => match snapshot.read(&desired) {
                    Ok(Some(node)) => if let Some(current) = node.current() {
                        // Simulated actuator: a real adapter publishes only after acting on its device.
                        let mut report = session.batch(); report.publish(&hue, &120, ExpiryUpdate::Clear)?.publish(&brightness, &current.value, ExpiryUpdate::Clear)?;
                        let receipt = report.commit().await?;
                        println!("simulated lamp report accepted at {}; warnings: {:?}", receipt.sequence, receipt.warnings);
                    },
                    Ok(None) => {},
                    Err(error) => eprintln!("intent decode failed: {error}"),
                },
                None => break,
            },
            result = tokio::signal::ctrl_c() => { if let Err(error) = result { eprintln!("signal error: {error}"); } break; }
        }
    }
    session.close().await
}
