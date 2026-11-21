//! Controls a simulated lamp: acts on desired brightness, reports actual state.
use tanuki_client::{ClaimRelease, ClientError, DesiredTopic, Request, Session, StateTopic};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), ClientError> {
    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "ws://127.0.0.1:5167/v1/ws".into());
    let session = Session::connect(&url, "example lamp controller").await?;
    let desired = DesiredTopic::<u8>::new("/lamp/desired-brightness")?;
    let hue = StateTopic::<u16>::new("/lamp/hue")?;
    let brightness = StateTopic::<u8>::new("/lamp/brightness")?;
    // Defines and claims the input, re-claiming it after any reconnect.
    let mut requests = session.control(&desired, ClaimRelease::Immediate).await?;
    loop {
        tokio::select! {
            request = requests.recv() => match request {
                Some(Ok(Request::Submitted(request))) => {
                    // A real adapter publishes only after its device reports back.
                    let receipt = session.batch()
                        .publish(&hue, &120)
                        .publish(&brightness, &request.value)
                        .commit()
                        .await?;
                    println!("{} asked for {}; reported at {}", request.last_write.client, request.value, receipt.sequence);
                }
                Some(Ok(Request::Cleared)) => println!("request withdrawn"),
                Some(Err(error)) => eprintln!("{error}"),
                None => break,
            },
            result = tokio::signal::ctrl_c() => {
                if let Err(error) = result { eprintln!("signal error: {error}"); }
                break;
            }
        }
    }
    session.close().await;
    Ok(())
}
