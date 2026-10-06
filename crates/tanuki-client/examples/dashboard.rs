use tanuki_client::protocol::{ClientName, Selector};
use tanuki_client::{ClientError, Session};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), ClientError> {
    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "ws://127.0.0.1:5167/v1/ws".into());
    let session = Session::connect(
        &url,
        ClientName::parse("example dashboard").expect("constant valid name"),
        vec![
            Selector::parse("/battery/*").unwrap(),
            Selector::parse("/lamp/**").unwrap(),
        ],
    )
    .await?;
    let battery = session.state::<u8>("/battery/laptop")?;
    let hue = session.state::<u16>("/lamp/hue")?;
    let brightness = session.state::<u8>("/lamp/brightness")?;
    let mut observer = session
        .observe_batch([
            battery.path().clone(),
            hue.path().clone(),
            brightness.path().clone(),
        ])
        .await?;
    loop {
        tokio::select! {
            snapshot = observer.recv() => match snapshot? {
                Some(snapshot) => {
                    println!("complete snapshot at {}", snapshot.sequence());
                    // Decode errors stay local to the value; raw nodes and metadata remain inspectable.
                    println!("battery: {:?}; hue: {:?}; brightness: {:?}", snapshot.read(&battery), snapshot.read(&hue), snapshot.read(&brightness));
                }
                None => break,
            },
            result = tokio::signal::ctrl_c() => { if let Err(error) = result { eprintln!("signal error: {error}"); } break; }
        }
    }
    session.close().await
}
